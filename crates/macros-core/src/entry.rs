//! Entry-key segments for macro-registered entry points.
//!
//! Entry keys are `<protocol>/<owner segment>.<handler segment>`. Both segments are derived
//! from Rust names or owner IDs by converting them to `snake_case`, so `listUsers`, `r#type`
//! and `"Calc Service"` all form keys (`list_users`, `type`, `calc_service`). Only a name that
//! has no such form, such as one with non-ASCII letters, is rejected.

use heck::ToSnakeCase;
use proc_macro2::Span;
use syn::ext::IdentExt;
use syn::{Ident, LitStr};

/// The entry-key segment naming a handler method: its name without a raw-identifier prefix,
/// in `snake_case`.
pub fn handler_segment(handler: &Ident) -> syn::Result<LitStr> {
    let name = handler.unraw().to_string();

    segment(&name, handler.span(), "name an entry")
}

/// The entry-key segment naming an entry owner (a service or controller): its stable ID with
/// every `/`-separated part in `snake_case`. The error spans the ID, or the type when the ID
/// is defaulted from it.
pub fn owner_segment(id: &LitStr) -> syn::Result<LitStr> {
    segment(&id.value(), id.span(), "identify an entry owner")
}

fn segment(name: &str, span: Span, role: &str) -> syn::Result<LitStr> {
    let parts = name
        .split('/')
        .map(ToSnakeCase::to_snake_case)
        .collect::<Vec<_>>();

    if !parts.iter().all(|part| is_segment(part)) {
        return Err(syn::Error::new(
            span,
            format!(
                "`{name}` cannot {role}: an entry key needs it in snake_case ASCII (lowercase \
                 letters, digits, `.`, `_` or `-`), and it has no such form"
            ),
        ));
    }

    Ok(LitStr::new(&parts.join("/"), span))
}

fn is_segment(name: &str) -> bool {
    let mut bytes = name.bytes();
    let starts_well = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());

    starts_well
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests;
