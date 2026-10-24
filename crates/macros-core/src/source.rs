//! Source-position emission shared by every descriptor macro.
//!
//! The emitted `file!()`/`line!()`/`column!()` carry the user token's span, so the compiler
//! resolves them at that token rather than at the macro invocation.

use proc_macro2::{Span, TokenStream};
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
