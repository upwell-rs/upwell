//! `#[config]` — implements the `ConfigProperties` trait for a config type and,
//! when given `#[config(path = "..")]`, auto-registers a binding into the
//! `CONFIG_BINDINGS` slice so `auto_discover` picks it up.
//!
//! `NAME` defaults to the type name (override with `#[config(name = "..")]`). The
//! type must also be `Deserialize`. Omitting `path` leaves binding to an explicit
//! `AppBuilder::config::<T>(path)` call — needed when the same type binds at
//! several paths.
//!
//! Applies to a `struct` or an `enum` (serde handles either). Fields may carry a
//! field-level `#[default = ".."]` whose value is a template string merged under the
//! config before deserialization, so a missing field falls back to a (possibly
//! templated) default that resolves through the normal `${..}` pipeline. On an enum, a
//! variant may be marked with a bare `#[default]` (or the cfg-gated
//! `#[cfg_attr(PRED, default)]`) to select it when the config names no variant.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Ident, Lit, LitStr, Meta, Token, Variant,
    meta::ParseNestedMeta,
    parse::{Parse, ParseStream},
    parse_quote,
    punctuated::Punctuated,
    token,
};

use crate::case::RenameRule;
use crate::paths::Paths;

/// Arguments of the `#[config(...)]` attribute on a config type.
#[derive(Default)]
pub struct ConfigArgs {
    name: Option<LitStr>,
    path: Option<LitStr>,
    /// Override for the core `upwell` facade root (`upwell = ::path`).
    upwell: Option<syn::Path>,
    /// Override for the plugin own-types root (`crate = ::path`).
    krate: Option<syn::Path>,
}

impl ConfigArgs {
    /// Resolves the crate [`Paths`] for this invocation: the macro's `default` roots with any
    /// `upwell =` / `crate =` overrides applied.
    pub fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), self.krate.clone())
    }
}

impl Parse for ConfigArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ConfigArgs::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            match key.to_string().as_str() {
                "name" => {
                    input.parse::<Token![=]>()?;
                    args.name = Some(input.parse()?);
                }
                "path" => {
                    input.parse::<Token![=]>()?;
                    args.path = Some(input.parse()?);
                }
                "upwell" => args.upwell = Some(crate::attr::parse_path_override(input)?),
                "crate" => args.krate = Some(crate::attr::parse_path_override(input)?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown argument `{other}`, expected `name`, `path`, `upwell`, or `crate`"
                        ),
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

pub fn expand(args: ConfigArgs, mut item: DeriveInput, paths: &Paths) -> syn::Result<TokenStream> {
    let ident = item.ident.clone();

    let name = args
        .name
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let config_properties = paths.core("ConfigProperties");
    let config_binding_descriptor = paths.core("ConfigBindingDescriptor");
    let config_bindings = paths.core("CONFIG_BINDINGS");
    let descriptor = paths.core("Descriptor");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let type_descriptor = paths.core("TypeDescriptor");

    // Collect (and strip) the field-level `#[default = ".."]` attributes, then build the
    // `const DEFAULTS`. Stripping is required so the sibling `#[derive(Deserialize)]`
    // does not see the unknown attribute.
    let defaults_const = build_defaults(&mut item, paths)?;

    // A baked-in path exposes the binding on the type as `Descriptor<ConfigBindingDescriptor>`
    // and auto-registers it into the `CONFIG_BINDINGS` slice (picked up by
    // `ConfigManager::auto_discover`). Without a path the binding is made explicitly at the
    // manager/builder (the multi-path case), so no descriptor is emitted.
    let registration = match args.path {
        Some(path) => {
            let condition_input = crate::when::config_input(&ident, &path, paths);

            quote! {
            impl #descriptor<#config_binding_descriptor> for #ident {
                const DESCRIPTOR: #config_binding_descriptor = #config_binding_descriptor {
                    ty: #type_descriptor::of::<#ident>(#name),
                    path: #path,
                    bind: <#ident as #config_properties>::bind,
                    slot: <#ident as #config_properties>::slot,
                    defaults: <#ident as #config_properties>::DEFAULTS,
                };
            }

            const _: () = {
                #[#distributed_slice(#config_bindings)]
                #[linkme(crate = #linkme_crate)]
                #[allow(unsafe_code)]
                static __UPWELL_CONFIG_BINDING: #config_binding_descriptor =
                    <#ident as #descriptor<#config_binding_descriptor>>::DESCRIPTOR;
            };

            #condition_input
            }
        }

        None => quote!(),
    };

    // The `ConfigProperties` impl and the `CONFIG_BINDINGS` (`linkme`) registration are the
    // server-side config machinery — gated out on wasm. The struct itself stays as a plain serde
    // DTO (harmless, and usable as shared data if a client references it).
    let server = crate::gate::native_only(quote! {
        impl #config_properties for #ident {
            const NAME: &'static str = #name;

            #defaults_const
        }

        #registration
    });

    Ok(quote! {
        // The user's config type is kept on every target (usable as a plain serde DTO on a wasm
        // client); only the `ConfigProperties`/`CONFIG_BINDINGS` wiring above is server-gated.
        #[cfg_attr(target_family = "wasm", allow(dead_code))]
        #item

        #server
    })
}

/// Builds the `const DEFAULTS: DefaultSpec = ..` associated constant from the type's
/// field-level `#[default = ".."]` attributes, stripping each consumed attribute from `item`.
///
/// Returns an empty token stream (so the trait default `DefaultSpec::None` stands) when no
/// field carries one. Every value is a string literal, so the spec is `const`-constructible
/// from `&'static` slices — no runtime allocation. A struct yields `DefaultSpec::Fields`; an
/// enum yields `DefaultSpec::Variants`, including only variants that have at least one
/// defaulted field.
fn build_defaults(item: &mut DeriveInput, paths: &Paths) -> syn::Result<TokenStream> {
    let default_spec = paths.core("DefaultSpec");

    // serde's container-level rename rules: `rename_all` renames a struct's fields or an
    // enum's variant tags; `rename_all_fields` (enum only) renames fields inside variants.
    let container_rename_all = serde_rename_all(&item.attrs, "rename_all")?;
    let container_rename_all_fields = serde_rename_all(&item.attrs, "rename_all_fields")?;
    // The enum's serde tagging, so the merge can synthesize the matching shape (unused for
    // structs).
    let enum_tagging = enum_tag_tokens(&item.attrs, paths)?;

    match &mut item.data {
        Data::Struct(data) => {
            let fields = take_field_defaults(&mut data.fields, container_rename_all)?;

            if fields.is_empty() {
                return Ok(quote!());
            }

            let entries = fields.iter().map(|(field, lit)| quote! { (#field, #lit) });

            Ok(quote! {
                const DEFAULTS: #default_spec = #default_spec::Fields(&[ #(#entries),* ]);
            })
        }

        Data::Enum(data) => {
            let mut variants = Vec::new();
            let mut default_variant: Option<(String, bool)> = None;

            for variant in data.variants.iter_mut() {
                let is_default = take_variant_default_marker(&mut variant.attrs)?;
                let variant_key = variant_serde_name(variant, container_rename_all)?;
                let is_unit = matches!(variant.fields, Fields::Unit);
                // A variant's own `rename_all` wins over the enum's `rename_all_fields`.
                let field_rule =
                    serde_rename_all(&variant.attrs, "rename_all")?.or(container_rename_all_fields);
                let fields = take_field_defaults(&mut variant.fields, field_rule)?;

                if is_default {
                    if default_variant.is_some() {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "a `#[config]` enum may mark at most one variant `#[default]`",
                        ));
                    }

                    default_variant = Some((variant_key.clone(), is_unit));
                }

                if !fields.is_empty() {
                    variants.push((variant_key, fields));
                }
            }

            if default_variant.is_none() && variants.is_empty() {
                return Ok(quote!());
            }

            let default_tokens = match &default_variant {
                Some((tag, is_unit)) => {
                    quote! { ::core::option::Option::Some((#tag, #is_unit)) }
                }

                None => quote! { ::core::option::Option::None },
            };
            let entries = variants.iter().map(|(variant, fields)| {
                let field_entries = fields.iter().map(|(field, lit)| quote! { (#field, #lit) });

                quote! {
                    (#variant, &[ #(#field_entries),* ])
                }
            });

            Ok(quote! {
                const DEFAULTS: #default_spec = #default_spec::Variants {
                    tagging: #enum_tagging,
                    default: #default_tokens,
                    fields: &[ #(#entries),* ],
                };
            })
        }

        Data::Union(data) => Err(syn::Error::new_spanned(
            data.union_token,
            "`#[config]` cannot be applied to a union",
        )),
    }
}

/// Pulls every `#[default = ".."]` off the named fields of `fields`, returning
/// `(serde field name, template literal)` pairs and removing the consumed attributes.
///
/// The key is the name serde *deserializes* into — a field `#[serde(rename = "..")]` wins,
/// otherwise `rule` (the applicable `rename_all`) transforms the identifier — so the default
/// lands under the same key as the file value. A default on an unnamed (tuple) field is
/// rejected: there is no field name to key the merged value by.
fn take_field_defaults(
    fields: &mut Fields,
    rule: Option<RenameRule>,
) -> syn::Result<Vec<(String, LitStr)>> {
    let mut defaults = Vec::new();

    match fields {
        Fields::Named(named) => {
            for field in named.named.iter_mut() {
                if let Some(lit) = take_default_attr(&mut field.attrs)? {
                    let ident = field
                        .ident
                        .as_ref()
                        .expect("named field has an identifier")
                        .to_string();
                    let key = field_serde_name(&field.attrs, &ident, rule)?;

                    defaults.push((key, lit));
                }
            }
        }

        Fields::Unnamed(unnamed) => {
            for field in unnamed.unnamed.iter_mut() {
                if take_default_attr(&mut field.attrs)?.is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "`#[default = \"..\"]` is only supported on named fields",
                    ));
                }
            }
        }

        Fields::Unit => {}
    }

    Ok(defaults)
}

/// The name serde deserializes a field into: an explicit `#[serde(rename = "..")]`, else the
/// identifier transformed by the applicable `rename_all` rule, else the identifier itself.
fn field_serde_name(
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
fn variant_serde_name(variant: &Variant, rule: Option<RenameRule>) -> syn::Result<String> {
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
fn serde_rename_all(attrs: &[Attribute], arg: &str) -> syn::Result<Option<RenameRule>> {
    let value = serde_string_arg(attrs, arg)?;

    Ok(value.and_then(|rule| RenameRule::from_str(&rule)))
}

/// Builds the `EnumTag` literal describing the type's serde enum representation, so the merge
/// can synthesize a default variant in the shape serde deserializes. Reads `#[serde(untagged)]`,
/// `#[serde(tag = "..")]`, and `#[serde(tag = "..", content = "..")]`; otherwise external.
fn enum_tag_tokens(attrs: &[Attribute], paths: &Paths) -> syn::Result<TokenStream> {
    let enum_tag = paths.core("EnumTag");

    if serde_flag(attrs, "untagged")? {
        return Ok(quote! { #enum_tag::Untagged });
    }

    let tag = serde_string_arg(attrs, "tag")?;
    let content = serde_string_arg(attrs, "content")?;

    let tokens = match (tag, content) {
        (Some(tag), Some(content)) => quote! {
            #enum_tag::Adjacent { tag: #tag, content: #content }
        },

        (Some(tag), None) => quote! {
            #enum_tag::Internal { tag: #tag }
        },

        _ => quote! { #enum_tag::External },
    };

    Ok(tokens)
}

/// Whether a bare `#[serde(<flag>)]` (e.g. `untagged`) is present on any serde attribute.
fn serde_flag(attrs: &[Attribute], flag: &str) -> syn::Result<bool> {
    let mut found = false;

    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(flag) {
                found = true;

                Ok(())
            } else {
                skip_meta_value(&meta)
            }
        })?;
    }

    Ok(found)
}

/// Extracts the deserialize-side string value of a serde argument `key`, scanning every
/// `#[serde(..)]` attribute. Handles both `key = ".."` and the granular
/// `key(deserialize = "..", serialize = "..")` form, and skips every other serde argument
/// (whatever its shape) so unrelated attributes never derail parsing.
fn serde_string_arg(attrs: &[Attribute], key: &str) -> syn::Result<Option<String>> {
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

/// Removes a single `#[default = ".."]` attribute from `attrs`, returning its string
/// literal. Errors when the attribute is present but not in the `= "literal"` form.
fn take_default_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<LitStr>> {
    let position = attrs
        .iter()
        .position(|attr| attr.path().is_ident("default"));

    let index = match position {
        Some(index) => index,
        None => return Ok(None),
    };

    let attr = attrs.remove(index);

    match &attr.meta {
        Meta::NameValue(nv) => match &nv.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(lit), ..
            }) => Ok(Some(lit.clone())),

            other => Err(syn::Error::new_spanned(
                other,
                "`#[default = ..]` expects a string literal template, e.g. `#[default = \"${tcp.ip}:8080\"]`",
            )),
        },

        other => Err(syn::Error::new_spanned(
            other,
            "`#[config]` field default must be written `#[default = \"..\"]`",
        )),
    }
}

/// Removes a `#[default]` marker from an enum variant's attributes, reporting whether it was
/// present. The marker selects the variant used when the config names none. Both the direct
/// `#[default]` and the cfg-gated `#[cfg_attr(PRED, default)]` form are recognized — the
/// latter so a platform-specific variant (e.g. a Unix socket) can be the default on its
/// platform. Errors on the field form `#[default = ".."]`, which is meaningless on a variant.
///
/// Note: the macro cannot evaluate the `cfg` predicate, so a `cfg_attr(PRED, default)` variant
/// is treated as the default regardless of `PRED`. Pair it with a `#[cfg(PRED)]` on the
/// variant itself (so the variant only exists where it is the default), as is conventional.
fn take_variant_default_marker(attrs: &mut Vec<Attribute>) -> syn::Result<bool> {
    if let Some(index) = attrs
        .iter()
        .position(|attr| attr.path().is_ident("default"))
    {
        if !matches!(attrs[index].meta, Meta::Path(_)) {
            return Err(syn::Error::new_spanned(
                &attrs[index].meta,
                "a variant's `#[default]` marker takes no value; `#[default = \"..\"]` is for fields",
            ));
        }

        attrs.remove(index);

        return Ok(true);
    }

    for index in 0..attrs.len() {
        if !attrs[index].path().is_ident("cfg_attr") {
            continue;
        }

        if let Some(rewritten) = take_default_from_cfg_attr(&attrs[index])? {
            match rewritten {
                Some(attr) => attrs[index] = attr,
                None => {
                    attrs.remove(index);
                }
            }

            return Ok(true);
        }
    }

    Ok(false)
}

/// Looks for a bare `default` among a `#[cfg_attr(pred, ..)]`'s conditional attributes.
///
/// Returns `Ok(None)` when this cfg_attr carries no `default`. Otherwise returns
/// `Ok(Some(rewritten))`: `Some(attr)` is the cfg_attr with `default` removed (other
/// conditional attributes preserved), or `None` when `default` was the only one (so the whole
/// cfg_attr is dropped). Stripping is required because, after the macro runs, the compiler
/// would expand the surviving `default` into an unknown attribute.
fn take_default_from_cfg_attr(attr: &Attribute) -> syn::Result<Option<Option<Attribute>>> {
    let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let mut iter = metas.into_iter();

    let predicate = match iter.next() {
        Some(predicate) => predicate,
        None => return Ok(None),
    };

    let mut remaining = Vec::new();
    let mut found = false;

    for meta in iter {
        if matches!(&meta, Meta::Path(path) if path.is_ident("default")) {
            found = true;
        } else {
            remaining.push(meta);
        }
    }

    if !found {
        return Ok(None);
    }

    if remaining.is_empty() {
        return Ok(Some(None));
    }

    let rebuilt: Attribute = parse_quote! { #[cfg_attr(#predicate, #(#remaining),*)] };

    Ok(Some(Some(rebuilt)))
}
