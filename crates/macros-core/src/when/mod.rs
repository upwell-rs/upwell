//! `#[when(<bool expr>)]`: component conditions written as ordinary Rust expressions.
//!
//! A `#[when]` sits **below** a component macro (`#[component]`, `#[service]`,
//! `#[controller]`), which runs first and consumes it: the expression lowers to a trusted
//! condition callback over the inputs derived from its syntax (see [`inputs`]), and stacked
//! attributes combine with AND. The component macro leaves a marker in place of each
//! consumed attribute, so the `when` attribute macro still runs, passes the item through,
//! and keeps the user's `when` import used. Reached any other way — without a component
//! macro or above one — the attribute macro reports a placement error.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Attribute, MacroDelimiter, Meta, MetaList};

mod args;
mod emit;
mod impls;
mod inputs;
mod purity;

pub(crate) use emit::Condition;
pub(crate) use impls::{component_input, config_input};

use args::{WhenAttr, is_lowered_marker, lowered_marker};

/// The attribute name a component macro consumes.
const WHEN_ATTRIBUTE: &str = "when";

/// Parses and consumes every `#[when]` in `attrs`, leaving the lowered marker in its place.
pub(crate) fn take(attrs: &mut [Attribute]) -> syn::Result<Vec<WhenAttr>> {
    let mut whens = Vec::new();
    let mut errors: Option<syn::Error> = None;

    for attr in attrs.iter_mut().filter(|attr| is_when(attr)) {
        match WhenAttr::parse(attr) {
            Ok(when) => whens.push(when),

            Err(error) => match &mut errors {
                Some(errors) => errors.combine(error),

                None => errors = Some(error),
            },
        }

        mark_lowered(attr);
    }

    match errors {
        Some(errors) => Err(errors),

        None => Ok(whens),
    }
}

/// The `#[when]` attribute macro: a pass-through for an attribute a component macro already
/// lowered, and a placement error otherwise.
pub fn expand_attr(attr: TokenStream, item: TokenStream) -> TokenStream {
    if is_lowered_marker(&attr) {
        return item;
    }

    let error = syn::Error::new(
        Span::call_site(),
        "`#[when]` must be placed below a component macro (`#[component]`, `#[service]` or `#[controller]`), which reads it",
    )
    .into_compile_error();

    quote! {
        #error

        #item
    }
}

fn is_when(attr: &Attribute) -> bool {
    let is_named_when = attr
        .path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == WHEN_ATTRIBUTE);
    let lowered = matches!(&attr.meta, Meta::List(list) if is_lowered_marker(&list.tokens));

    is_named_when && !lowered
}

fn mark_lowered(attr: &mut Attribute) {
    let path = attr.path().clone();

    attr.meta = Meta::List(MetaList {
        path,
        delimiter: MacroDelimiter::Paren(Default::default()),
        tokens: lowered_marker(),
    });
}

#[cfg(test)]
mod tests;
