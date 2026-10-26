//! `#[entry]` expansion: registers a free function as an entry into a scope.
//!
//! The function is kept unchanged. Beside it the macro emits a unit marker type named after the
//! function in `UpperCamelCase`, which carries the entry's `EntryDescriptor` (so the builder can
//! register it explicitly) and appends that descriptor to the `ENTRIES` discovery slice. Every
//! parameter is an injected target and must be a `FromContainer` type, the same parameters an
//! `#[init]` constructor takes.

use heck::ToUpperCamelCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{FnArg, ItemFn, LitStr, Token, Type};

use super::{handler_segment, is_segment};
use crate::paths::Paths;

/// The namespace of every key an `#[entry]` derives from its function name.
const DEFAULT_NAMESPACE: &str = "app";

/// The arguments of `#[entry(scope = S, key = "..")]`.
pub struct EntryArgs {
    scope: syn::Path,
    key: Option<LitStr>,
    upwell: Option<syn::Path>,
    krate: Option<syn::Path>,
}

impl EntryArgs {
    pub(crate) fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), self.krate.clone())
    }
}

impl Parse for EntryArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut scope = None;
        let mut key = None;
        let mut upwell = None;
        let mut krate = None;

        while !input.is_empty() {
            let name = syn::Ident::parse_any(input)?;

            match name.to_string().as_str() {
                "scope" => {
                    input.parse::<Token![=]>()?;
                    scope = Some(input.parse()?);
                }
                "key" => {
                    input.parse::<Token![=]>()?;
                    key = Some(validated_key(input.parse()?)?);
                }
                "upwell" => upwell = Some(crate::attr::parse_path_override(input)?),
                "crate" => krate = Some(crate::attr::parse_path_override(input)?),
                _ => {
                    return Err(syn::Error::new_spanned(
                        name,
                        "unknown argument; expected one of: `scope`, `key`, `upwell`, `crate`",
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let scope = scope.ok_or_else(|| {
            input.error("#[entry] needs the scope it runs in, as `#[entry(scope = MyScope)]`")
        })?;

        Ok(Self {
            scope,
            key,
            upwell,
            krate,
        })
    }
}

/// Expands `#[entry]` on `item`.
pub fn expand(args: EntryArgs, item: ItemFn, paths: &Paths) -> syn::Result<TokenStream> {
    let targets = target_types(&item)?;
    let ident = &item.sig.ident;
    let marker = marker_ident(ident)?;
    let key = match args.key {
        Some(key) => key,
        None => default_key(ident)?,
    };

    let descriptor_trait = paths.core("Descriptor");
    let entry_descriptor = paths.core("EntryDescriptor");
    let entries = paths.core("ENTRIES");
    let entry_key = paths.core("entry_key");
    let from_container = paths.core("FromContainer");
    let scope_trait = paths.core("Scope");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");

    let vis = &item.vis;
    let scope = &args.scope;
    let scope_key = quote_spanned!(scope.span()=> <#scope as #scope_trait>::KEY);
    let target_edges = targets
        .iter()
        .map(|ty| quote_spanned!(ty.span()=> <#ty as #from_container>::dependency()));
    let source = crate::source::item_source(ident.span(), &ident.unraw().to_string(), paths);
    let di_assert = di_assertion(&targets, paths);
    let doc = format!("The entry point declared by [`{}`].", ident.unraw());

    Ok(quote! {
        #item

        #[doc = #doc]
        #vis struct #marker;

        impl #descriptor_trait<#entry_descriptor> for #marker {
            const DESCRIPTOR: #entry_descriptor = #entry_descriptor {
                key: #entry_key!(#key),
                scope: #scope_key,
                targets: || ::std::vec![#(#target_edges),*],
                owner: ::core::option::Option::None,
                source: ::core::option::Option::Some(#source),
            };
        }

        const _: () = {
            let _ = #ident;

            #[#distributed_slice(#entries)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static __UPWELL_ENTRY: #entry_descriptor =
                <#marker as #descriptor_trait<#entry_descriptor>>::DESCRIPTOR;
        };

        #di_assert
    })
}

/// The parameter types of `item`, each an injected target. A receiver or a generic parameter
/// is rejected, since an entry's targets must be concrete types.
fn target_types(item: &ItemFn) -> syn::Result<Vec<&Type>> {
    if !item.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.sig.generics,
            "an #[entry] function cannot be generic: its parameters are the concrete targets the \
             entry injects",
        ));
    }

    item.sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(typed) => Ok(&*typed.ty),
            FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(
                receiver,
                "an #[entry] is a free function and cannot take `self`",
            )),
        })
        .collect()
}

/// The marker type named after the function, spanned at its name. A function already named in
/// `UpperCamelCase` would share the marker's value namespace, so it is rejected.
fn marker_ident(ident: &syn::Ident) -> syn::Result<syn::Ident> {
    let name = ident.unraw().to_string();
    let marker = name.to_upper_camel_case();

    if marker == name {
        return Err(syn::Error::new_spanned(
            ident,
            format!(
                "#[entry] names its marker type `{marker}` after this function; name the \
                 function in snake_case"
            ),
        ));
    }

    Ok(format_ident!("{marker}", span = ident.span()))
}

/// `app/<function name in snake_case>`, spanned at the function name.
fn default_key(ident: &syn::Ident) -> syn::Result<LitStr> {
    let segment = handler_segment(ident)?;

    Ok(LitStr::new(
        &format!("{DEFAULT_NAMESPACE}/{}", segment.value()),
        ident.span(),
    ))
}

/// Checks an explicit `key = ".."` against the namespaced-identifier grammar, so a malformed
/// key points at the literal instead of failing const evaluation.
fn validated_key(key: LitStr) -> syn::Result<LitStr> {
    let value = key.value();
    let segments = value.split('/').collect::<Vec<_>>();

    if segments.len() < 2 || !segments.iter().all(|segment| is_segment(segment)) {
        return Err(syn::Error::new_spanned(
            &key,
            format!(
                "`{value}` is not a valid entry key: it needs at least two `/`-separated \
                 segments of lowercase ASCII letters, digits, `.`, `_` or `-`, each starting \
                 with a letter or digit"
            ),
        ));
    }

    Ok(key)
}

/// An uncalled assertion that every checked target is provided, each bound spanned at its
/// parameter type so a missing provider points there. Empty unless `di-check` is on or no
/// target is checked.
fn di_assertion(targets: &[&Type], paths: &Paths) -> TokenStream {
    let bounds = targets
        .iter()
        .filter_map(|ty| checked_bound(ty, paths))
        .collect::<Vec<_>>();

    if !crate::di::enabled() || bounds.is_empty() {
        return quote!();
    }

    quote! {
        const _: () = {
            fn __upwell_assert_di()
            where
                #(#bounds,)*
            {
            }
        };
    }
}

/// The `Wiring: Provided<..>` bound asserted for a required, concrete `Arc<T>` target under
/// `di-check`. Trait-object, optional, dynamic, config and collection targets are left to
/// preparation, as they are for component fields.
fn checked_bound(ty: &Type, paths: &Paths) -> Option<TokenStream> {
    let injectable = paths.core("Injectable");
    let provided = paths.core("Provided");
    let wiring = paths.core("Wiring");
    let inner = crate::attr::arc_inner_type(ty).ok()?;

    if matches!(inner, Type::TraitObject(_)) {
        return None;
    }

    let bound = quote!(#wiring: #provided<<#ty as #injectable>::Target>);

    Some(crate::source::respan(bound, ty.span()))
}

#[cfg(test)]
mod tests;
