//! Source-position emission shared by every descriptor macro.
//!
//! The emitted `file!()`/`line!()`/`column!()` carry the user token's span, so the compiler
//! resolves them at that token rather than at the macro invocation.

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::quote_spanned;

use crate::paths::Paths;

/// A `DescriptorSource` expression for the position of `span`.
pub fn descriptor_source(span: Span, paths: &Paths) -> TokenStream {
    let source_ty = paths.core("DescriptorSource");

    quote_spanned!(span=> #source_ty::new(::core::file!(), ::core::line!(), ::core::column!()))
}

/// An `ItemSource` expression for the item named `item` (`Type` or `Type::method`) whose
/// identifier sits at `span`, in the module the macro expands into.
pub fn item_source(span: Span, item: &str, paths: &Paths) -> TokenStream {
    let item_source_ty = paths.core("ItemSource");
    let location = descriptor_source(span, paths);

    quote_spanned!(span=> #item_source_ty::new(#location, ::core::module_path!(), #item))
}

/// A `TypeDescriptor` expression for the type `ty`, declared by the identifier at `ident`.
pub fn declared_type(
    ty: &impl quote::ToTokens,
    ident: &syn::Ident,
    name: &impl quote::ToTokens,
    paths: &Paths,
) -> TokenStream {
    let type_descriptor = paths.core("TypeDescriptor");
    let source = item_source(ident.span(), &ident.to_string(), paths);

    quote::quote!(#type_descriptor::declared::<#ty>(#name, #source))
}

/// The in-module path of an associated item: `Type::method`, with `self_ty` rendered without
/// token spacing (`Users<T>::list`).
pub fn associated_item(self_ty: &syn::Type, item: &syn::Ident) -> String {
    let ty = quote::quote!(#self_ty).to_string().replace(' ', "");

    format!("{ty}::{item}")
}

/// Locates every token of `tokens` at `span`, keeping each token's own name resolution, so
/// diagnostics about generated code point at the user's token rather than at the macro.
pub fn respan(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|mut token| {
            if let TokenTree::Group(group) = &token {
                let mut respanned =
                    proc_macro2::Group::new(group.delimiter(), respan(group.stream(), span));

                respanned.set_span(group.span().located_at(span));
                token = TokenTree::Group(respanned);
            } else {
                token.set_span(token.span().located_at(span));
            }

            token
        })
        .collect()
}
