use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::atomic::{AtomicUsize, Ordering},
};
use syn::{
    Attribute, DeriveInput, FnArg, GenericArgument, GenericParam, ItemTrait, Lifetime,
    LifetimeParam, Meta, Path, PathArguments, ReturnType, Token, TraitBoundModifier, TraitItem,
    TraitItemFn, Type, TypeParamBound, parse_macro_input, parse_quote,
    punctuated::Punctuated,
    visit_mut::{self, VisitMut},
};

static SCHEMA_IDS: AtomicUsize = AtomicUsize::new(0);

pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match expand_reflect(input) {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_reflect(input: DeriveInput) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "Reflect currently requires a concrete, non-generic type",
        ));
    }

    let type_name = &input.ident;
    let automatic_traits: [Path; 4] = [
        parse_quote!(gpui::Styled),
        parse_quote!(gpui::InteractiveElement),
        parse_quote!(gpui::StatefulInteractiveElement),
        parse_quote!(gpui::ParentElement),
    ];
    let mut explicit_traits = Vec::new();

    for attribute in &input.attrs {
        if attribute.path().is_ident("reflect") {
            explicit_traits.extend(
                attribute.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?,
            );
        }
    }

    let probes = automatic_traits.iter().map(|trait_path| {
        quote! {
            {
                struct Probe<Type>(::std::marker::PhantomData<Type>);

                trait Detect {
                    fn register(self, implementations: &mut Vec<gpui::reflection::ReflectedImplementation>);
                }

                impl<Type> Detect for &Probe<Type> {
                    fn register(self, _implementations: &mut Vec<gpui::reflection::ReflectedImplementation>) {}
                }

                impl<Type: #trait_path + 'static> Detect for &&Probe<Type> {
                    fn register(self, implementations: &mut Vec<gpui::reflection::ReflectedImplementation>) {
                        #trait_path.__register::<Type>(implementations);
                    }
                }

                (&&Probe::<#type_name>(::std::marker::PhantomData)).register(&mut implementations);
            }
        }
    });
    let explicit = explicit_traits.iter().map(|trait_path| {
        quote! { #trait_path.__register::<#type_name>(&mut implementations); }
    });

    Ok(quote! {
        impl gpui::reflection::Reflect for #type_name {
            fn reflected_traits() -> Vec<gpui::reflection::ReflectedTrait> {
                gpui::reflection::registered_traits(::std::any::TypeId::of::<Self>()).to_vec()
            }
        }

        gpui::private::inventory::submit! {
            gpui::reflection::ReflectionRegistration {
                type_id: || ::std::any::TypeId::of::<#type_name>(),
                implementations: || {
                    let mut implementations = Vec::new();
                    #(#probes)*
                    #(#explicit)*

                    implementations
                },
            }
        }
    })
}

pub fn reflect_trait(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            Span::call_site(),
            "reflect_trait takes no arguments; inheritance comes from the trait declaration",
        )
        .to_compile_error()
        .into();
    }

    let input = parse_macro_input!(input as ItemTrait);

    match expand_trait(input) {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_trait(input: ItemTrait) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "reflect_trait requires a non-generic trait without a where clause",
        ));
    }

    if input.unsafety.is_some() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "unsafe traits cannot be forwarded through reflected elements",
        ));
    }

    let name = &input.ident;
    let visibility = &input.vis;
    let marker = format_ident!("__GpuiReflect{name}");
    let group = format_ident!("__GpuiReflect{name}Group");
    let methods = format_ident!("__GpuiReflect{name}Methods");
    let collect_name = format_ident!("__GpuiReflect{name}Collect");
    let schema = schema_path(&parse_quote!(#name));
    let schema_name = &schema.segments.last().unwrap().ident;
    let mut origin = DefaultHasher::new();
    std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_default()
        .hash(&mut origin);
    std::env::var("CARGO_CRATE_NAME")
        .unwrap_or_default()
        .hash(&mut origin);
    let export_name = format_ident!(
        "__gpui_reflect_schema_{}_{:x}_{}",
        name,
        origin.finish(),
        SCHEMA_IDS.fetch_add(1, Ordering::Relaxed),
    );
    let mut parents = Vec::new();

    for bound in &input.supertraits {
        let TypeParamBound::Trait(bound) = bound else {
            return Err(syn::Error::new_spanned(
                bound,
                "reflected supertraits must be non-generic trait paths",
            ));
        };

        if bound.path.is_ident("Sized") {
            continue;
        }

        if bound.modifier != TraitBoundModifier::None
            || bound.lifetimes.is_some()
            || bound
                .path
                .segments
                .iter()
                .any(|segment| !matches!(segment.arguments, PathArguments::None))
        {
            return Err(syn::Error::new_spanned(
                bound,
                "reflected supertraits must be non-generic trait paths",
            ));
        }

        parents.push(bound.path.clone());
    }

    let parent_aliases = (0..parents.len())
        .map(|idx| format_ident!("Parent{idx}"))
        .collect::<Vec<_>>();
    let parent_imports = (0..parents.len())
        .map(|idx| format_ident!("__GpuiReflect{name}Parent{idx}"))
        .collect::<Vec<_>>();
    let parent_schemas = parents.iter().map(schema_path);
    let mut forwarded = Vec::new();

    for item in &input.items {
        match item {
            TraitItem::Fn(method) if method.default.is_none() => {
                forwarded.push(forward_method(method, name)?);
            }
            TraitItem::Type(_) => {
                return Err(syn::Error::new_spanned(
                    item,
                    "reflected traits cannot have associated types",
                ));
            }
            TraitItem::Const(item) if item.default.is_none() => {
                return Err(syn::Error::new_spanned(
                    item,
                    "reflected traits cannot have required associated constants",
                ));
            }
            _ => {}
        }
    }

    let fields = forwarded.iter().map(|method| &method.field);
    let initializers = forwarded.iter().map(|method| &method.initializer);
    let implementations = forwarded.iter().map(|method| &method.implementation);
    let parent_members = parents.iter().zip(&parent_aliases).map(|(parent, alias)| {
        quote! {
            __GpuiReflectionGroup: gpui::reflection::IncludesReflectedTrait<#schema_name::#alias::Marker>,
            gpui::reflection::ReflectedElement<__GpuiReflectionGroup>: #parent,
        }
    });
    let configurations = configuration_attributes(&input.attrs);
    let parent_import_items = parent_schemas.zip(&parent_imports).map(|(schema, import)| {
        quote! {
            #(#configurations)*
            #[doc(hidden)]
            pub use #schema as #import;
        }
    });

    Ok(quote! {
        #input

        #(#configurations)*
        #[doc(hidden)]
        #[derive(Clone, Copy, Default)]
        pub struct #marker;

        #(#configurations)*
        #[doc(hidden)]
        pub struct #methods { #(#fields)* }

        #(#parent_import_items)*

        #(#configurations)*
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub mod #schema_name {
            pub type Marker = super::#marker;
            #(pub use super::#parent_imports as #parent_aliases;)*
            pub use super::#collect_name as collect;
        }

        #(#configurations)*
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #export_name {
            (
                [$mode:ident $($group:tt)*]
                [$($schema:tt)*]
                [$($pending:tt)*]
                [$($members:tt)*]
                [$($roots:tt)*]
            ) => {
                ::gpui::reflection::__collect_reflected_traits! {
                    [$mode $($group)*]
                    [#export_name]
                    [$($schema)*::Marker]
                    [#([$($schema)*::#parent_aliases])*]
                    [$($pending)*]
                    [$($members)*]
                    [$($roots)*]
                }
            };
        }

        #(#configurations)*
        #[doc(hidden)]
        pub use #export_name as #collect_name;

        #(#configurations)*
        #schema_name::collect! { [items pub #group] [#schema_name] [] [] [] }

        #(#configurations)*
        impl gpui::reflection::ReflectionToken for #marker {
            type Group = #group;
            type Methods = #methods;

            fn reflected_trait(self) -> gpui::reflection::ReflectedTrait {
                gpui::reflection::ReflectedTrait::new(
                    concat!(module_path!(), "::", stringify!(#name)),
                    || ::std::any::TypeId::of::<#marker>(),
                ).with_supertraits(|| {
                    static PARENTS: ::std::sync::LazyLock<Vec<gpui::reflection::ReflectedTrait>>
                        = ::std::sync::LazyLock::new(|| vec![
                            #(gpui::reflection::ReflectionToken::reflected_trait(
                                #schema_name::#parent_aliases::Marker::default(),
                            )),*
                        ]);

                    &PARENTS
                })
            }
        }

        #(#configurations)*
        impl #marker {
            #[doc(hidden)]
            #[allow(private_bounds)]
            pub fn __register<Type: #name + 'static>(
                self,
                implementations: &mut Vec<gpui::reflection::ReflectedImplementation>,
            ) {
                let descriptor = gpui::reflection::ReflectionToken::reflected_trait(self);

                if implementations.iter().any(|implementation| implementation.descriptor == descriptor) {
                    return;
                }

                implementations.push(gpui::reflection::ReflectedImplementation {
                    descriptor,
                    methods: Box::new(#methods { #(#initializers)* }),
                });

                #(#schema_name::#parent_aliases::Marker::default()
                    .__register::<Type>(implementations);)*
            }
        }

        #(#configurations)*
        impl<__GpuiReflectionGroup> #name for gpui::reflection::ReflectedElement<__GpuiReflectionGroup>
        where
            __GpuiReflectionGroup: gpui::reflection::IncludesReflectedTrait<#marker>,
            #(#parent_members)*
        {
            #(#implementations)*
        }

        #(#configurations)*
        #[doc = concat!("Descriptor for the ", stringify!(#name), " trait.")]
        #[allow(non_upper_case_globals)]
        #visibility const #name: #marker = #marker;
    })
}

pub(crate) fn schema_path(path: &Path) -> Path {
    let mut schema = path.clone();
    let segment = schema.segments.last_mut().unwrap();
    segment.ident = format_ident!("__GpuiReflect{}Schema", segment.ident);

    schema
}

fn configuration_attributes(attributes: &[Attribute]) -> Vec<Attribute> {
    attributes
        .iter()
        .filter_map(|attribute| {
            if attribute.path().is_ident("cfg") {
                return Some(attribute.clone());
            }

            if !attribute.path().is_ident("cfg_attr") {
                return None;
            }

            let arguments = attribute
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .ok()?;
            let mut arguments = arguments.iter();
            let condition = arguments.next()?;
            let configurations = arguments
                .filter(|argument| argument.path().is_ident("cfg"))
                .collect::<Vec<_>>();

            if configurations.is_empty() {
                return None;
            }

            Some(parse_quote!(#[cfg_attr(#condition, #(#configurations),*)]))
        })
        .collect()
}

struct ForwardedMethod {
    field: TokenStream2,
    initializer: TokenStream2,
    implementation: TokenStream2,
}

fn forward_method(method: &TraitItemFn, trait_name: &syn::Ident) -> syn::Result<ForwardedMethod> {
    let signature = &method.sig;
    let method_name = &signature.ident;

    if signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || signature.generics.where_clause.is_some()
        || signature.generics.params.iter().any(|parameter| {
            !matches!(parameter, GenericParam::Lifetime(parameter) if parameter.bounds.is_empty())
        })
    {
        return Err(syn::Error::new_spanned(
            signature,
            "required reflected methods support borrowed receivers, lifetime parameters, and concrete types",
        ));
    }

    let Some(FnArg::Receiver(receiver)) = signature.inputs.first() else {
        return Err(syn::Error::new_spanned(
            signature,
            "required reflected methods must have an &self or &mut self receiver",
        ));
    };
    let Some((_, lifetime)) = &receiver.reference else {
        return Err(syn::Error::new_spanned(
            receiver,
            "required reflected methods must have an &self or &mut self receiver",
        ));
    };

    if receiver.colon_token.is_some() {
        return Err(syn::Error::new_spanned(
            receiver,
            "typed receivers are not supported",
        ));
    }

    let lifetime = lifetime.as_ref().filter(|lifetime| lifetime.ident != "_");
    let element_lifetime = lifetime
        .cloned()
        .unwrap_or_else(|| Lifetime::new("'__gpui_element", Span::mixed_site()));
    let mut lifetimes = signature.generics.params.clone();

    if lifetime.is_none() {
        lifetimes.push(GenericParam::Lifetime(LifetimeParam::new(
            element_lifetime.clone(),
        )));
    }

    let mut signature = signature.clone();
    let mut erased_arguments = Vec::new();
    let mut argument_names = Vec::new();
    let mut preparations = Vec::new();

    for (idx, argument) in signature.inputs.iter_mut().skip(1).enumerate() {
        let FnArg::Typed(argument) = argument else {
            unreachable!()
        };
        let argument_name = format_ident!("argument_{idx}");
        argument.pat = Box::new(parse_quote!(#argument_name));
        argument_names.push(argument_name.clone());

        if let Type::ImplTrait(iterator) = argument.ty.as_ref() {
            let bounds = iterator.bounds.iter().collect::<Vec<_>>();
            let [TypeParamBound::Trait(bound)] = bounds.as_slice() else {
                return Err(syn::Error::new_spanned(
                    iterator,
                    "only impl IntoIterator<Item = ConcreteType> can be erased",
                ));
            };
            let segment = bound.path.segments.last().unwrap();
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return Err(syn::Error::new_spanned(
                    iterator,
                    "an iterator Item type is required",
                ));
            };
            let arguments = arguments.args.iter().collect::<Vec<_>>();
            let [GenericArgument::AssocType(item)] = arguments.as_slice() else {
                return Err(syn::Error::new_spanned(
                    iterator,
                    "an iterator Item type is required",
                ));
            };

            if segment.ident != "IntoIterator"
                || item.ident != "Item"
                || bound.modifier != TraitBoundModifier::None
                || bound.lifetimes.is_some()
            {
                return Err(syn::Error::new_spanned(
                    iterator,
                    "only impl IntoIterator<Item = ConcreteType> can be erased",
                ));
            }

            validate_type(&item.ty)?;
            let item_type = &item.ty;
            erased_arguments.push(quote! { &mut dyn ::std::iter::Iterator<Item = #item_type> });
            preparations.push(quote! { let mut #argument_name = #argument_name.into_iter(); });

            continue;
        }

        validate_type(&argument.ty)?;
        let argument_type = &argument.ty;
        erased_arguments.push(quote! { #argument_type });
    }

    let mut output = signature.output.clone();

    if let ReturnType::Type(_, output_type) = &mut output {
        validate_type(output_type)?;
        OutputLifetimes(&element_lifetime).visit_type_mut(output_type);
    }

    let erased_receiver = if receiver.mutability.is_some() {
        quote! { &#element_lifetime mut dyn ::std::any::Any }
    } else {
        quote! { &#element_lifetime dyn ::std::any::Any }
    };
    let downcast = if receiver.mutability.is_some() {
        quote! { downcast_mut }
    } else {
        quote! { downcast_ref }
    };
    let parts = if receiver.mutability.is_some() {
        quote! { __reflection_parts_mut }
    } else {
        quote! { __reflection_parts }
    };
    let forwarded_arguments = signature.inputs.iter().skip(1).zip(&argument_names).map(|(argument, name)| {
        if matches!(argument, FnArg::Typed(argument) if matches!(*argument.ty, Type::ImplTrait(_))) {
            return quote! { &mut #name };
        }

        quote! { #name }
    });
    let configurations = configuration_attributes(&method.attrs);
    let lifetime_binder = if lifetimes.is_empty() {
        quote! {}
    } else {
        quote! { for<#lifetimes> }
    };

    Ok(ForwardedMethod {
        field: quote! {
            #(#configurations)*
            #method_name: #lifetime_binder fn(#erased_receiver, #(#erased_arguments),*) #output,
        },
        initializer: quote! {
            #(#configurations)*
            #method_name: |element, #(#argument_names),*| {
                let concrete = element.#downcast::<Type>().expect("reflected element type changed");

                <Type as #trait_name>::#method_name(concrete, #(#argument_names),*)
            },
        },
        implementation: quote! {
            #(#configurations)*
            #signature {
                #(#preparations)*
                let (methods, element) = self.#parts(#trait_name);

                (methods.#method_name)(element, #(#forwarded_arguments),*)
            }
        },
    })
}

fn validate_type(type_name: &Type) -> syn::Result<()> {
    struct UnsupportedType(Option<syn::Error>);

    impl VisitMut for UnsupportedType {
        fn visit_type_mut(&mut self, type_name: &mut Type) {
            if matches!(type_name, Type::ImplTrait(_))
                || matches!(type_name, Type::Path(path) if path.path.segments.iter().any(|segment| segment.ident == "Self"))
            {
                self.0 = Some(syn::Error::new_spanned(
                    type_name,
                    "required reflected methods cannot use Self or impl Trait outside a supported iterator argument",
                ));

                return;
            }

            visit_mut::visit_type_mut(self, type_name);
        }
    }

    let mut visitor = UnsupportedType(None);
    visitor.visit_type_mut(&mut type_name.clone());

    match visitor.0 {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct OutputLifetimes<'life>(&'life Lifetime);

impl VisitMut for OutputLifetimes<'_> {
    fn visit_type_bare_fn_mut(&mut self, _function: &mut syn::TypeBareFn) {}

    fn visit_parenthesized_generic_arguments_mut(
        &mut self,
        _arguments: &mut syn::ParenthesizedGenericArguments,
    ) {
    }

    fn visit_type_reference_mut(&mut self, reference: &mut syn::TypeReference) {
        if reference
            .lifetime
            .as_ref()
            .is_none_or(|lifetime| lifetime.ident == "_")
        {
            reference.lifetime = Some(self.0.clone());
        }

        visit_mut::visit_type_reference_mut(self, reference);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_signatures_that_cannot_be_forwarded() {
        for input in [
            parse_quote!(
                trait Generic<Item> {}
            ),
            parse_quote!(
                trait Associated {
                    type Item;
                }
            ),
            parse_quote!(
                trait Owned {
                    fn consume(self);
                }
            ),
            parse_quote!(
                trait ReturningSelf {
                    fn copy(&self) -> Self;
                }
            ),
            parse_quote!(
                trait GenericMethod {
                    fn update<Type>(&mut self, value: Type);
                }
            ),
            parse_quote!(
                trait AsyncMethod {
                    async fn update(&mut self);
                }
            ),
        ] {
            assert!(expand_trait(input).is_err());
        }
    }
}
