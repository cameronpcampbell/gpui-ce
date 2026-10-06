use crate::derive_reflect::schema_path;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use std::collections::VecDeque;
use syn::{
    Ident, Path, Token, Visibility, bracketed,
    parse::{Parse, ParseStream, Parser},
    punctuated::Punctuated,
};

pub fn trait_set(input: TokenStream) -> TokenStream {
    let parser = Punctuated::<Path, Token![,]>::parse_terminated;

    match parser.parse(input).and_then(expand_trait_set) {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_trait_set(traits: Punctuated<Path, Token![,]>) -> syn::Result<TokenStream2> {
    if traits.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "trait_set requires at least one trait",
        ));
    }

    let mut schemas = traits.iter().map(schema_path);
    let first = schemas.next().unwrap();
    let pending = schemas.map(|schema| quote! { [[#schema] any] });
    let group = format_ident!("__GpuiReflectionGroup", span = Span::mixed_site());

    Ok(quote! {
        #first::collect! {
            [expression #group]
            [[#first] any]
            [#(#pending)*]
            []
            []
            [#traits]
        }
    })
}

pub fn collect_traits(input: TokenStream) -> TokenStream {
    match syn::parse::<Collection>(input).and_then(expand_collection) {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

struct Collection {
    mode: Ident,
    visibility: Visibility,
    group: Ident,
    key: Ident,
    marker: Path,
    kind: MemberKind,
    requires_callable: bool,
    parents: VecDeque<PendingVisit>,
    pending: VecDeque<PendingVisit>,
    members: Vec<CollectedMember>,
    witnesses: Vec<IdentityWitness>,
    roots: Punctuated<Path, Token![,]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberKind {
    Membership,
    Callable,
}

impl Parse for MemberKind {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kind: Ident = input.parse()?;

        match kind.to_string().as_str() {
            "membership" => Ok(Self::Membership),
            "callable" => Ok(Self::Callable),
            _other => Err(syn::Error::new_spanned(
                kind,
                "expected membership or callable reflection mode",
            )),
        }
    }
}

impl ToTokens for MemberKind {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let kind = match self {
            Self::Membership => quote! { membership },
            Self::Callable => quote! { callable },
        };

        tokens.extend(kind);
    }
}

struct CollectedMember {
    transport_key: Ident,
    marker: Path,
    kind: MemberKind,
}

impl ToTokens for CollectedMember {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let Self {
            transport_key,
            marker,
            kind,
        } = self;

        tokens.extend(quote! { [#transport_key [#marker] #kind] });
    }
}

struct PendingVisit {
    schema: Path,
    requires_callable: bool,
}

fn callable_expectation(input: ParseStream) -> syn::Result<bool> {
    let expectation: Ident = input.parse()?;

    match expectation.to_string().as_str() {
        "any" => Ok(false),
        "callable" => Ok(true),
        _other => Err(syn::Error::new_spanned(
            expectation,
            "expected any or callable reflection expectation",
        )),
    }
}

impl ToTokens for PendingVisit {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let schema = &self.schema;
        let expectation = if self.requires_callable {
            quote! { callable }
        } else {
            quote! { any }
        };

        tokens.extend(quote! { [[#schema] #expectation] });
    }
}

struct IdentityWitness {
    retained: Path,
    encountered: Path,
}

impl ToTokens for IdentityWitness {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let Self {
            retained,
            encountered,
        } = self;

        tokens.extend(quote! { [[#retained] [#encountered]] });
    }
}

fn visits(input: ParseStream) -> syn::Result<VecDeque<PendingVisit>> {
    let list;
    bracketed!(list in input);
    let mut visits = VecDeque::new();

    while !list.is_empty() {
        let visit;
        bracketed!(visit in list);
        let path;
        bracketed!(path in visit);
        visits.push_back(PendingVisit {
            schema: path.parse()?,
            requires_callable: callable_expectation(&visit)?,
        });
    }

    Ok(visits)
}

impl Parse for Collection {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let header;
        bracketed!(header in input);
        let mode = header.parse()?;
        let visibility = header.parse()?;
        let group = header.parse()?;

        let key;
        bracketed!(key in input);
        let key = key.parse()?;
        let marker;
        bracketed!(marker in input);
        let marker = marker.parse()?;

        let kind;
        bracketed!(kind in input);
        let kind = kind.parse()?;
        let expectation;
        bracketed!(expectation in input);
        let requires_callable = callable_expectation(&expectation)?;

        let parents = visits(input)?;
        let pending = visits(input)?;

        let members_input;
        bracketed!(members_input in input);
        let mut members = Vec::new();

        while !members_input.is_empty() {
            let member;
            bracketed!(member in members_input);
            let transport_key = member.parse()?;
            let marker;
            bracketed!(marker in member);
            members.push(CollectedMember {
                transport_key,
                marker: marker.parse()?,
                kind: member.parse()?,
            });
        }

        let witnesses_input;
        bracketed!(witnesses_input in input);
        let mut witnesses = Vec::new();

        while !witnesses_input.is_empty() {
            let witness;
            bracketed!(witness in witnesses_input);
            let retained;
            bracketed!(retained in witness);
            let encountered;
            bracketed!(encountered in witness);
            witnesses.push(IdentityWitness {
                retained: retained.parse()?,
                encountered: encountered.parse()?,
            });
        }

        let roots;
        bracketed!(roots in input);
        let roots = roots.parse_terminated(Path::parse, Token![,])?;

        Ok(Self {
            mode,
            visibility,
            group,
            key,
            marker,
            kind,
            requires_callable,
            parents,
            pending,
            members,
            witnesses,
            roots,
        })
    }
}

fn expand_collection(mut collection: Collection) -> syn::Result<TokenStream2> {
    if collection.requires_callable && collection.kind != MemberKind::Callable {
        return Err(syn::Error::new_spanned(
            collection.marker,
            "callable reflected parents require callable reflection",
        ));
    }

    let retained = collection
        .members
        .iter()
        .find(|member| member.transport_key == collection.key);

    if let Some(retained) = retained {
        if retained.kind != collection.kind {
            return Err(syn::Error::new_spanned(
                collection.key,
                "reflected trait transport key reported inconsistent member kinds",
            ));
        }

        collection.witnesses.push(IdentityWitness {
            retained: retained.marker.clone(),
            encountered: collection.marker,
        });
    } else {
        collection.members.push(CollectedMember {
            transport_key: collection.key,
            marker: collection.marker,
            kind: collection.kind,
        });
        collection.parents.append(&mut collection.pending);
        collection.pending = collection.parents;
    }

    let mode = collection.mode;
    let visibility = collection.visibility;
    let group = collection.group;
    let roots = collection.roots;
    let members = &collection.members;
    let witnesses = &collection.witnesses;

    if let Some(next) = collection.pending.pop_front() {
        let schema = &next.schema;
        let pending = collection.pending.iter();

        return Ok(quote! {
            #schema::collect! {
                [#mode #visibility #group]
                #next
                [#(#pending)*]
                [#(#members)*]
                [#(#witnesses)*]
                [#roots]
            }
        });
    }

    let markers = collection
        .members
        .iter()
        .map(|member| &member.marker)
        .collect::<Vec<_>>();
    let callable_markers = collection
        .members
        .iter()
        .filter(|member| member.kind == MemberKind::Callable)
        .map(|member| &member.marker);
    let identity_checks = witnesses.iter().enumerate().map(|(idx, witness)| {
        let name = format_ident!(
            "__GPUI_REFLECTION_{group}_IDENTITY_CHECK_{idx}",
            span = Span::mixed_site(),
        );
        let IdentityWitness {
            retained,
            encountered,
        } = witness;

        quote! {
            #[allow(non_upper_case_globals)]
            const #name: fn(#retained) -> #encountered = |token| token;
        }
    });
    let items = quote! {
        #[doc(hidden)]
        #visibility struct #group;

        impl ::gpui::reflection::ReflectionGroup for #group {}

        #(impl ::gpui::reflection::IncludesReflectedTrait<#markers> for #group {})*

        #(impl ::gpui::reflection::IncludesCallableTrait<#callable_markers> for #group {})*

        #(#identity_checks)*
    };

    if mode == "items" {
        return Ok(items);
    }

    let root_checks = roots.iter().enumerate().map(|(idx, root)| {
        let name = format_ident!(
            "__GPUI_REFLECTION_{group}_ROOT_CHECK_{idx}",
            span = Span::mixed_site(),
        );
        let schema = schema_path(root);

        quote! {
            #[allow(non_upper_case_globals)]
            const #name: #schema::Marker = #root;
        }
    });
    let roots = roots.iter();

    Ok(quote! {{
        #items

        #(#root_checks)*

        ::gpui::reflection::ReflectedTraitGroup::<#group>::with_requirements(
            [#(::gpui::reflection::ReflectionToken::requirements(#roots)),*]
                .into_iter().flatten(),
        )
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_trait_sets() {
        let error = expand_trait_set(Punctuated::new()).unwrap_err();

        assert_eq!(error.to_string(), "trait_set requires at least one trait");
    }

    #[test]
    fn preserves_parent_order_and_duplicate_identity_checks() {
        let first: Collection = syn::parse2(quote! {
            [expression SelectedGroup]
            [child_key] [Child::Marker] [callable] [any]
            [[[FirstParent] callable] [[SecondParent] callable]]
            [[[Root] any]]
            [] [] [Child, Root]
        })
        .unwrap();
        let first = expand_collection(first).unwrap();
        let expected = quote! {
            FirstParent::collect! {
                [expression SelectedGroup]
                [[FirstParent] callable]
                [[[SecondParent] callable] [[Root] any]]
                [[child_key [Child::Marker] callable]]
                [] [Child, Root]
            }
        };

        assert_eq!(first.to_string(), expected.to_string());

        let repeated: Collection = syn::parse2(quote! {
            [expression SelectedGroup]
            [child_key] [Alias::Marker] [callable] [callable]
            [[[AlreadyVisitedParent] callable]]
            [[[Root] any]]
            [[child_key [Child::Marker] callable]]
            [[[Prior::Marker] [PriorAlias::Marker]]]
            [Child, Root]
        })
        .unwrap();
        let repeated = expand_collection(repeated).unwrap();
        let expected = quote! {
            Root::collect! {
                [expression SelectedGroup]
                [[Root] any]
                []
                [[child_key [Child::Marker] callable]]
                [
                    [[Prior::Marker] [PriorAlias::Marker]]
                    [[Child::Marker] [Alias::Marker]]
                ]
                [Child, Root]
            }
        };

        assert_eq!(repeated.to_string(), expected.to_string());
    }

    #[test]
    fn rejects_invalid_callable_edges_and_inconsistent_member_modes() {
        let cases = [
            (
                quote! {
                    [items SelectedGroup]
                    [parent_key] [Parent::Marker] [membership] [callable]
                    [] [] [[parent_key [Parent::Marker] membership]] [] []
                },
                "callable reflected parents require callable reflection",
            ),
            (
                quote! {
                    [items SelectedGroup]
                    [shared_key] [Second::Marker] [callable] [any]
                    [] [] [[shared_key [First::Marker] membership]] [] []
                },
                "reflected trait transport key reported inconsistent member kinds",
            ),
        ];

        for (input, expected) in cases {
            let collection: Collection = syn::parse2(input).unwrap();
            let error = expand_collection(collection).unwrap_err();

            assert_eq!(error.to_string(), expected);
        }
    }
}
