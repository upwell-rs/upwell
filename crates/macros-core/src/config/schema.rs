//! The `ConfigSchema` constant: a config type's fields, variants, docs, defaults, and
//! declaration positions, for tooling.
//!
//! Built from the unmodified item, before `#[default]` attributes are consumed, so it reports
//! exactly the defaults and serde names the type deserializes with.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Attribute, Data, DeriveInput, Expr, ExprLit, Field, Fields, Lit, Meta, Type, Variant};

use super::defaults::take_variant_default_marker;
use super::serde_attrs::{field_serde_name, serde_flag, serde_rename_all, variant_serde_name};
use crate::case::RenameRule;
use crate::paths::Paths;

/// The `const SCHEMA: ConfigSchema = ..` item for the `ConfigProperties` impl.
pub(super) fn build_schema(item: &DeriveInput, paths: &Paths) -> syn::Result<TokenStream> {
    let schema = paths.core("schema::ConfigSchema");
    let docs = doc_text(&item.attrs);
    let source = crate::source::item_source(item.ident.span(), &item.ident.to_string(), paths);
    let container_default = serde_flag(&item.attrs, "default")?;
    let shape = match &item.data {
        Data::Struct(data) => {
            let rule = serde_rename_all(&item.attrs, "rename_all")?;
            let fields = field_schemas(&data.fields, rule, container_default, paths)?;
            let shape = paths.core("schema::ConfigShape");

            quote!(#shape::Struct(&[ #(#fields),* ]))
        }

        Data::Enum(data) => enum_shape(item, data.variants.iter(), paths)?,

        Data::Union(data) => {
            return Err(syn::Error::new_spanned(
                data.union_token,
                "`#[config]` cannot be applied to a union",
            ));
        }
    };

    Ok(quote! {
        const SCHEMA: #schema = #schema {
            docs: #docs,
            source: ::core::option::Option::Some(#source),
            shape: #shape,
        };
    })
}

/// The `DescribedValue` impl that lets a field of this type carry its declared schema.
pub(super) fn described_value_impl(item: &DeriveInput, paths: &Paths) -> TokenStream {
    let ident = &item.ident;
    let described_value = paths.core("schema::DescribedValue");
    let value_schema = paths.core("schema::ValueSchema");
    let config_value_schema = paths.core("schema::config_value_schema");

    quote! {
        impl #described_value for #ident {
            fn describe() -> #value_schema {
                #config_value_schema::<Self>()
            }
        }
    }
}

fn enum_shape<'a>(
    item: &DeriveInput,
    variants: impl Iterator<Item = &'a Variant>,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    let shape = paths.core("schema::ConfigShape");
    let tagging = super::defaults::enum_tag_tokens(&item.attrs, paths)?;
    let rename_all = serde_rename_all(&item.attrs, "rename_all")?;
    let rename_all_fields = serde_rename_all(&item.attrs, "rename_all_fields")?;
    let mut schemas = Vec::new();

    for variant in variants {
        if is_skipped(&variant.attrs)? {
            continue;
        }

        schemas.push(variant_schema(
            variant,
            rename_all,
            rename_all_fields,
            paths,
        )?);
    }

    Ok(quote!(#shape::Enum { tagging: #tagging, variants: &[ #(#schemas),* ] }))
}

fn variant_schema(
    variant: &Variant,
    rename_all: Option<RenameRule>,
    rename_all_fields: Option<RenameRule>,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    let variant_schema = paths.core("schema::VariantSchema");
    let payload_ty = paths.core("schema::VariantPayload");
    let name = variant_serde_name(variant, rename_all)?;
    let ident = variant.ident.to_string();
    let docs = doc_text(&variant.attrs);
    let default = take_variant_default_marker(&mut variant.attrs.clone())?;
    let source = crate::source::descriptor_source(variant.ident.span(), paths);
    let payload = match &variant.fields {
        Fields::Unit => quote!(#payload_ty::Unit),

        Fields::Named(_) => {
            let rule = serde_rename_all(&variant.attrs, "rename_all")?.or(rename_all_fields);
            let fields = field_schemas(&variant.fields, rule, false, paths)?;

            quote!(#payload_ty::Struct(&[ #(#fields),* ]))
        }

        Fields::Unnamed(unnamed) => {
            let values = unnamed
                .unnamed
                .iter()
                .map(|field| value_probe(&field.ty, paths));

            quote!(#payload_ty::Tuple(&[ #(#values),* ]))
        }
    };

    Ok(quote! {
        #variant_schema {
            name: #name,
            ident: #ident,
            docs: #docs,
            default: #default,
            source: #source,
            payload: #payload,
        }
    })
}

/// The `FieldSchema` literals of the named, deserialized fields in `fields`.
fn field_schemas(
    fields: &Fields,
    rule: Option<RenameRule>,
    container_default: bool,
    paths: &Paths,
) -> syn::Result<Vec<TokenStream>> {
    let Fields::Named(named) = fields else {
        return Ok(Vec::new());
    };
    let mut schemas = Vec::new();

    for field in &named.named {
        if is_skipped(&field.attrs)? {
            continue;
        }

        schemas.push(field_schema(field, rule, container_default, paths)?);
    }

    Ok(schemas)
}

fn field_schema(
    field: &Field,
    rule: Option<RenameRule>,
    container_default: bool,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    let field_schema = paths.core("schema::FieldSchema");
    let ident = field.ident.as_ref().expect("named field has an identifier");
    let ident_text = ident.to_string();
    let name = field_serde_name(&field.attrs, &ident_text, rule)?;
    let docs = doc_text(&field.attrs);
    let ty = &field.ty;
    let declared_type = type_text(ty);
    let default = default_template(&field.attrs);
    let required = default.is_none()
        && !container_default
        && !serde_flag(&field.attrs, "default")?
        && !is_option(ty);
    let default = match default {
        Some(template) => quote!(::core::option::Option::Some(#template)),
        None => quote!(::core::option::Option::None),
    };
    let flatten = serde_flag(&field.attrs, "flatten")?;
    let source = crate::source::descriptor_source(ident.span(), paths);
    let value = value_probe(ty, paths);

    Ok(quote! {
        #field_schema {
            name: #name,
            ident: #ident_text,
            docs: #docs,
            declared_type: #declared_type,
            rust_type: ::core::any::type_name::<#ty>,
            default: #default,
            required: #required,
            flatten: #flatten,
            source: #source,
            value: #value,
        }
    })
}

/// A `fn() -> ValueSchema` describing `ty` at the richest available level: its declared
/// schema, its traced `Deserialize` shape, or an opaque value.
///
/// The `&&&` receiver selects the level by autoref ranking, which `needless_borrow` would
/// otherwise flag in the user's crate.
fn value_probe(ty: &Type, paths: &Paths) -> TokenStream {
    let probe = paths.core("schema::__probe");
    let value_schema = paths.core("schema::ValueSchema");

    quote! {
        (|| {
            use #probe::{ViaDescribed as _, ViaOpaque as _, ViaSerde as _};

            #[allow(clippy::needless_borrow)]
            let schema = (&&&#probe::Probe::<#ty>::NEW).value_schema();

            schema
        }) as fn() -> #value_schema
    }
}

/// Whether serde never deserializes the item (`skip` / `skip_deserializing`).
fn is_skipped(attrs: &[Attribute]) -> syn::Result<bool> {
    Ok(serde_flag(attrs, "skip")? || serde_flag(attrs, "skip_deserializing")?)
}

/// The item's `///` documentation, one line per doc attribute, common leading space removed.
fn doc_text(attrs: &[Attribute]) -> String {
    let lines = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(meta) => match &meta.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let text = lines
        .iter()
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");

    text.trim().to_string()
}

/// The `#[default = ".."]` template on a field, read without consuming it.
fn default_template(attrs: &[Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|attr| attr.path().is_ident("default"))
        .and_then(|attr| match &attr.meta {
            Meta::NameValue(meta) => match &meta.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
}

/// Whether `ty` is written as an `Option<..>`, which serde treats as not required.
fn is_option(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };

    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Option")
}

/// `ty` as source-like text: `Option<Vec<String>>` rather than token-spaced output.
fn type_text(ty: &Type) -> String {
    let text = ty.to_token_stream().to_string();

    [
        (" :: ", "::"),
        (":: ", "::"),
        (" < ", "<"),
        ("< ", "<"),
        (" <", "<"),
        (" >", ">"),
        (" ,", ","),
        ("& ", "&"),
    ]
    .iter()
    .fold(text, |text, (from, to)| text.replace(from, to))
}

#[cfg(test)]
mod tests;
