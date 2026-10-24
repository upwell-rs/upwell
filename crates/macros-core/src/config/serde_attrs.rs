//! The deserialize-side serde attributes `#[config]` mirrors: renames, rename rules, and flags.

use proc_macro2::TokenStream;
use syn::{Attribute, Expr, LitStr, Token, Variant, meta::ParseNestedMeta, token};

use crate::case::RenameRule;

/// The name serde deserializes a field into: an explicit `#[serde(rename = "..")]`, else the
/// identifier transformed by the applicable `rename_all` rule, else the identifier itself.
pub(super) fn field_serde_name(
    attrs: &[Attribute],
    ident: &str,
    rule: Option<RenameRule>,
) -> syn::Result<String> {
    if let Some(name) = serde_rename(attrs)? {
        return Ok(name);
    }

    let name = match rule {
        Some(rule) => rule.apply_to_field(ident),
        None => ident.to_string(),
    };

    Ok(name)
}

/// The tag serde deserializes a variant from: an explicit `#[serde(rename = "..")]`, else the
/// identifier transformed by the enum's `rename_all` rule, else the identifier itself.
pub(super) fn variant_serde_name(
    variant: &Variant,
    rule: Option<RenameRule>,
) -> syn::Result<String> {
    if let Some(name) = serde_rename(&variant.attrs)? {
        return Ok(name);
    }

    let ident = variant.ident.to_string();
    let name = match rule {
        Some(rule) => rule.apply_to_variant(&ident),
        None => ident,
    };

    Ok(name)
}

/// The deserialize-side `#[serde(rename = "..")]` value, if any (also reads the granular
/// `rename(deserialize = "..")` form).
fn serde_rename(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    serde_string_arg(attrs, "rename")
}

/// The deserialize-side rename rule for the named serde argument (`rename_all` or
/// `rename_all_fields`), if present and recognized.
pub(super) fn serde_rename_all(attrs: &[Attribute], arg: &str) -> syn::Result<Option<RenameRule>> {
    let value = serde_string_arg(attrs, arg)?;

    Ok(value.and_then(|rule| RenameRule::from_str(&rule)))
}

/// Whether `#[serde(<flag>)]` (e.g. `untagged`, or `default` with or without a `= "path"`
/// value) is present on any serde attribute.
pub(super) fn serde_flag(attrs: &[Attribute], flag: &str) -> syn::Result<bool> {
    let mut found = false;

    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(flag) {
                found = true;
            }

            skip_meta_value(&meta)
        })?;
    }

    Ok(found)
}

/// Extracts the deserialize-side string value of a serde argument `key`, scanning every
/// `#[serde(..)]` attribute. Handles both `key = ".."` and the granular
/// `key(deserialize = "..", serialize = "..")` form, and skips every other serde argument
/// (whatever its shape) so unrelated attributes never derail parsing.
pub(super) fn serde_string_arg(attrs: &[Attribute], key: &str) -> syn::Result<Option<String>> {
    let mut found = None;

    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident(key) {
                return skip_meta_value(&meta);
            }

            if let Ok(value) = meta.value() {
                let lit: LitStr = value.parse()?;
                found = Some(lit.value());
            } else {
                meta.parse_nested_meta(|inner| {
                    let is_deserialize = inner.path.is_ident("deserialize");
                    let lit: LitStr = inner.value()?.parse()?;

                    if is_deserialize {
                        found = Some(lit.value());
                    }

                    Ok(())
                })?;
            }

            Ok(())
        })?;
    }

    Ok(found)
}

/// Consumes an unrelated serde argument's payload — an `= value` or a balanced `(..)` group,
/// or nothing for a bare flag — so `parse_nested_meta` can advance to the next argument.
fn skip_meta_value(meta: &ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        let _: Token![=] = meta.input.parse()?;
        let _: Expr = meta.input.parse()?;
    } else if meta.input.peek(token::Paren) {
        let content;
        syn::parenthesized!(content in meta.input);
        let _: TokenStream = content.parse()?;
    }

    Ok(())
}
