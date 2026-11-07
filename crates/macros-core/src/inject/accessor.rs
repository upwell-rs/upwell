//! The field plan of an invocation-context accessor, `Current<T>` or `Scoped<S, T>`.
//!
//! An accessor holds nothing: it is built without resolving anything, and its edge (the
//! accessor's own `FromContainer::dependency`) is validated but never planned at
//! construction.

use proc_macro2::Literal;
use quote::quote;
use syn::{Attribute, Type};

use super::field::{FieldPlan, ReadyField};
use crate::paths::Paths;

/// Why an accessor field rejects `#[qualifier]`.
const QUALIFIER_UNSUPPORTED: &str = "`#[qualifier]` is not supported on a `Current<..>` or \
    `Scoped<..>` field: an accessor resolves its target's unqualified provider from the \
    active invocation; inject the qualified provider directly instead";

/// Why an accessor field rejects `#[config]`.
const CONFIG_UNSUPPORTED: &str = "`#[config]` is not supported on a `Current<..>` or \
    `Scoped<..>` field: config bindings are injected as `Cfg<T>`";

/// The plan of the accessor field of type `ty`, or an error at the `#[qualifier]` or
/// `#[config]` attribute it does not support.
pub(super) fn plan(
    ty: &Type,
    qualifier: Option<&Attribute>,
    config: Option<&Attribute>,
    binding: &syn::Ident,
    index: &Literal,
    paths: &Paths,
) -> syn::Result<FieldPlan> {
    let from_container = paths.core("FromContainer");

    if let Some(qualifier) = qualifier {
        return Err(syn::Error::new_spanned(qualifier, QUALIFIER_UNSUPPORTED));
    }

    if let Some(config) = config {
        return Err(syn::Error::new_spanned(config, CONFIG_UNSUPPORTED));
    }

    Ok(FieldPlan {
        value: quote!(<#ty as #from_container>::from_container(cx).await?),
        ready: ReadyField::read(
            binding,
            quote!(<#ty as #from_container>::ready_at(cx, #index)),
            quote!(#binding?),
        ),
        dependency: Some(quote!(<#ty as #from_container>::dependency())),
        check: None,
        wired: None,
    })
}
