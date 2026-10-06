use crate::derive_reflect::schema_path;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::collections::{HashSet, VecDeque};
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
    let pending = schemas.map(|schema| quote! { [#schema] });
    let group = format_ident!("__GpuiReflectionGroup", span = Span::mixed_site());

    Ok(quote! {
        #first::collect! {
            [expression #group]
            [#first]
            [#(#pending)*]
            []
            [#traits]
        }
    })
}

pub fn collect_traits(input: TokenStream) -> TokenStream {
    match syn::parse::<Collection>(input).map(expand_collection) {
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
    parents: VecDeque<Path>,
    pending: VecDeque<Path>,
    members: Vec<(Ident, Path)>,
    roots: Punctuated<Path, Token![,]>,
}

fn paths(input: ParseStream) -> syn::Result<VecDeque<Path>> {
    let list;
    bracketed!(list in input);
    let mut paths = VecDeque::new();

    while !list.is_empty() {
        let path;
        bracketed!(path in list);
        paths.push_back(path.parse()?);
    }

    Ok(paths)
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
        let parents = paths(input)?;
        let pending = paths(input)?;
        let members_input;
        bracketed!(members_input in input);
        let mut members = Vec::new();

        while !members_input.is_empty() {
            let member;
            bracketed!(member in members_input);
            let key = member.parse()?;
            let marker;
            bracketed!(marker in member);
            members.push((key, marker.parse()?));
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
            parents,
            pending,
            members,
            roots,
        })
    }
}

fn expand_collection(mut collection: Collection) -> TokenStream2 {
    let seen = collection
        .members
        .iter()
        .map(|(key, _marker)| key.to_string())
        .collect::<HashSet<_>>();

    if !seen.contains(&collection.key.to_string()) {
        collection.members.push((collection.key, collection.marker));
        collection.parents.append(&mut collection.pending);
        collection.pending = collection.parents;
    }

    let mode = collection.mode;
    let visibility = collection.visibility;
    let group = collection.group;
    let roots = collection.roots;
    let members = collection
        .members
        .iter()
        .map(|(key, marker)| quote! { [#key [#marker]] });

    if let Some(next) = collection.pending.pop_front() {
        let pending = collection.pending.iter().map(|schema| quote! { [#schema] });

        return quote! {
            #next::collect! {
                [#mode #visibility #group]
                [#next]
                [#(#pending)*]
                [#(#members)*]
                [#roots]
            }
        };
    }

    let markers = collection
        .members
        .iter()
        .map(|(_key, marker)| marker)
        .collect::<Vec<_>>();
    let items = quote! {
        #[doc(hidden)]
        #visibility struct #group;

        impl ::gpui::reflection::ReflectionGroup for #group {}

        #(impl ::gpui::reflection::IncludesReflectedTrait<#markers> for #group {})*

        #(impl ::gpui::reflection::IncludesCallableTrait<#markers> for #group {})*
    };

    if mode == "items" {
        return items;
    }

    let roots = roots.iter();

    quote! {{
        #items

        ::gpui::reflection::ReflectedTraitGroup::<#group>::with_requirements(
            [#(::gpui::reflection::ReflectionToken::requirements(#roots)),*]
                .into_iter().flatten(),
        )
    }}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_trait_sets() {
        let error = expand_trait_set(Punctuated::new()).unwrap_err();

        assert_eq!(error.to_string(), "trait_set requires at least one trait");
    }
}
