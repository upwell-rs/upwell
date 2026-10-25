//! Entry-key segments for macro-registered entry points.

use syn::ext::IdentExt;
use syn::{Ident, LitStr};

/// The entry-key segment naming a handler method: its name without a raw-identifier prefix.
///
/// Entry keys are `<protocol>/<owner ID>.<handler>`, so the name must itself be a valid key
/// segment: lowercase ASCII letters, digits, `.`, `_` or `-`, starting with a letter or
/// digit. Anything else is rejected here, at the method, instead of failing the key's
/// constant evaluation.
pub fn handler_segment(handler: &Ident) -> syn::Result<LitStr> {
    let name = handler.unraw().to_string();

    if !is_segment(&name) {
        return Err(syn::Error::new(
            handler.span(),
            format!(
                "`{name}` cannot name an entry: an entry key uses the handler name, which must \
                 start with a lowercase ASCII letter or digit and contain only lowercase ASCII \
                 letters, digits, `.`, `_` or `-`; rename it in snake_case"
            ),
        ));
    }

    Ok(LitStr::new(&name, handler.span()))
}

/// Validates the stable ID of an entry owner (a service or controller), which entry keys
/// embed as `<protocol>/<owner ID>.<handler>`.
///
/// Every `/`-separated part of the ID must be a valid key segment, as for
/// [`handler_segment`]. The error spans the ID, or the type when the ID is defaulted from it.
pub fn owner_id(id: &LitStr) -> syn::Result<()> {
    let value = id.value();

    if !value.split('/').all(is_segment) {
        return Err(syn::Error::new(
            id.span(),
            format!(
                "`{value}` cannot identify an entry owner: entry keys embed this ID, whose \
                 `/`-separated parts must start with a lowercase ASCII letter or digit and \
                 contain only lowercase ASCII letters, digits, `.`, `_` or `-`; set a valid \
                 `id = \"..\"`"
            ),
        ));
    }

    Ok(())
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
