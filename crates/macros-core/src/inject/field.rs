//! Per-field planning for the field-injection factory: how each field is built on the async
//! and synchronous paths, and the dependency edge it declares.

use proc_macro2::{Literal, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::spanned::Spanned;
use syn::{Expr, ExprLit, Field, Lit, LitStr, Meta};

use crate::attr;
use crate::paths::Paths;

/// One field's contribution: the value expression that builds it in the struct
/// literal, and the dependency descriptor it registers (absent for `#[default]`
/// local state).
pub(super) struct FieldPlan {
    pub(super) value: TokenStream,
    pub(super) ready: ReadyField,
    pub(super) dependency: Option<TokenStream>,
    /// The `Provided<Target>` type to assert eagerly for this field, when it is a
    /// required *concrete* dependency (trait-object / optional / dynamic / multi
    /// edges are not eagerly asserted in the per-macro path).
    pub(super) check: Option<TokenStream>,
    /// The `Provided<Target>` type for the lazy `Wired` bound — every required
    /// single dependency, *including* trait objects (checked only via `app!`).
    pub(super) wired: Option<TokenStream>,
}

/// How a field is built on the synchronous factory path: `read` binds a dependency before
/// the struct literal, returning `Ok(None)` when it is not ready, and `value` builds the
/// field. Reading every dependency first keeps a `#[default]` field from being built for an
/// attempt that falls back to the async path.
pub(super) struct ReadyField {
    pub(super) read: Option<TokenStream>,
    pub(super) value: TokenStream,
}

impl ReadyField {
    /// A field built the same way on both paths, without awaiting.
    fn same(value: &TokenStream) -> Self {
        Self {
            read: None,
            value: value.clone(),
        }
    }

    /// A field whose dependency `source` yields `Option<T>`, `None` meaning not ready.
    /// `value` builds the field from the read binding.
    fn read(binding: &syn::Ident, source: TokenStream, value: TokenStream) -> Self {
        Self {
            read: Some(quote! {
                let ::core::option::Option::Some(#binding) = #source else {
                    return ::core::result::Result::Ok(::core::option::Option::None);
                };
            }),
            value,
        }
    }
}

/// Classifies a field and, as a side effect, strips the `#[default]` marker so
/// the emitted struct stays valid.
///
/// `index` is the position the field's dependency edge takes in the factory's declared
/// edges, so a planned edge is read by position instead of by type.
pub(super) fn plan_field(field: &mut Field, index: usize, paths: &Paths) -> FieldPlan {
    let had_default = field.attrs.iter().any(|a| a.path().is_ident("default"));
    field.attrs.retain(|a| !a.path().is_ident("default"));

    if had_default {
        let value = quote!(::core::default::Default::default());

        return FieldPlan {
            ready: ReadyField::same(&value),
            value,
            dependency: None,
            check: None,
            wired: None,
        };
    }

    // `#[qualifier = ".."]` selects a specific provider for a single trait edge.
    let field_qualifier = field.attrs.iter().find_map(|a| {
        if a.path().is_ident("qualifier")
            && let Meta::NameValue(nv) = &a.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
        {
            return Some(s.clone());
        }

        None
    });
    field.attrs.retain(|a| !a.path().is_ident("qualifier"));

    // `#[config]` (sole-binding shorthand) or `#[config("app.db.reader")]` (path)
    // marks a config-binding injection.
    let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
    let is_config = config_attr.is_some();
    let config_path: Option<LitStr> = config_attr.and_then(|a| match &a.meta {
        Meta::List(list) => syn::parse2::<LitStr>(list.tokens.clone()).ok(),
        _ => None,
    });
    field.attrs.retain(|a| !a.path().is_ident("config"));

    let cardinality = paths.core("Cardinality");
    let dynamic_ty = paths.core("Dynamic");
    let error = paths.core("DiError");
    let injectable = paths.core("Injectable");
    let type_descriptor = paths.core("TypeDescriptor");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let resolution_mode = paths.core("ResolutionMode");
    let from_container = paths.core("FromContainer");
    let config_store = paths.core("ConfigStore");
    let resolver_ctx_ext = paths.core("ResolverCtxExt");

    let dependency_observation = paths.core("DependencyObservation");
    let dep = |handle: &syn::Type,
               kind: TokenStream,
               optional: bool,
               dynamic: bool,
               qualifier: TokenStream| {
        let dep_name_str = attr::arc_inner_type(handle)
            .ok()
            .or_else(|| attr::dep_inner(handle))
            .map_or_else(
                || handle.to_token_stream().to_string(),
                |inner| inner.to_token_stream().to_string(),
            );
        let dep_name = LitStr::new(&dep_name_str, handle.span());
        let observation = if attr::dep_inner(handle).is_some() {
            quote!(#dependency_observation::Live)
        } else {
            quote!(#dependency_observation::Snapshot)
        };

        quote! {
            #dependency_descriptor {
                name: #dep_name,
                ty: #type_descriptor::of::<<#handle as #injectable>::Target>(#dep_name),
                cardinality: #kind,
                optional: #optional,
                dynamic: #dynamic,
                qualifier: #qualifier,
                config: false,
                resolution: #resolution_mode::Eager,
                observation: #observation,
            }
        }
    };

    let none = quote!(::core::option::Option::None);
    let binding = format_ident!("__upwell_dep{index}");
    let index = Literal::usize_unsuffixed(index);

    let ty = &field.ty;

    if let Some(handle) = attr::fresh_inner(ty) {
        let qualifier = match &field_qualifier {
            Some(qualifier) => quote!(::core::option::Option::Some(#qualifier)),
            None => none.clone(),
        };
        let dependency = quote!({
            let mut dependency = <#ty as #from_container>::dependency();
            dependency.qualifier = #qualifier;
            dependency
        });

        let value = quote!(cx.fresh::<#handle>(#qualifier));

        return FieldPlan {
            ready: ReadyField::same(&value),
            value,
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    if let Some(target) = attr::deferred_inner(ty) {
        let qualifier = match &field_qualifier {
            Some(qualifier) => quote!(::core::option::Option::Some(#qualifier)),
            None => none.clone(),
        };
        let dependency = quote!({
            let mut dependency = <#ty as #from_container>::dependency();
            dependency.qualifier = #qualifier;
            dependency
        });

        let value = quote!(cx.deferred::<#target>(#qualifier)?);

        return FieldPlan {
            ready: ReadyField::same(&value),
            value,
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    if let Some(handle) = attr::lazy_inner(ty) {
        let dependency = match &field_qualifier {
            Some(qualifier) => quote!({
                let mut dependency = <#ty as #from_container>::dependency();
                dependency.qualifier = ::core::option::Option::Some(#qualifier);
                dependency
            }),
            None => quote!(<#ty as #from_container>::dependency()),
        };
        let (value, ready) = match (&field_qualifier, attr::arc_inner_type(&handle)) {
            (Some(qualifier), Ok(target)) => {
                let value = quote!(cx.lazy_qualified::<#target>(#qualifier));

                (value.clone(), ReadyField::same(&value))
            }
            _ => (
                quote!(<#ty as #from_container>::from_container(cx).await?),
                ReadyField::read(
                    &binding,
                    quote!(<#ty as #from_container>::ready_at(cx, #index)),
                    quote!(#binding?),
                ),
            ),
        };

        return FieldPlan {
            value,
            ready,
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    // A `#[config]` / `#[config("path")]` field is a config binding injected as
    // `Cfg<T>`, keyed by property path. Config edges are validated against the
    // registered bindings (not the component graph), so they emit no di-check
    // assertion.
    if is_config {
        let handle = ty.clone();
        let dep_name_str = match attr::cfg_inner(&handle) {
            Some(inner) => inner.to_token_stream().to_string(),
            None => handle.to_token_stream().to_string(),
        };
        let dep_name = LitStr::new(&dep_name_str, handle.span());

        // Config lives outside the container in a `ConfigStore` resolver, reached through
        // the construction context's resolver set. `get_resolver` is called by UFCS so the
        // generated code needs no `use` of the extension trait.
        let (resolved, qualifier) = match &config_path {
            Some(path) => (
                quote!(
                    #resolver_ctx_ext::get_resolver::<#config_store>(cx)
                        .and_then(|store| store.resolve_path::<#handle>(#path))
                ),
                quote!(::core::option::Option::Some(#path)),
            ),
            None => (
                quote!(
                    #resolver_ctx_ext::get_resolver::<#config_store>(cx)
                        .and_then(|store| store.resolve_sole::<#handle>())
                ),
                none.clone(),
            ),
        };

        let dependency = quote! {
            #dependency_descriptor {
                name: #dep_name,
                ty: #type_descriptor::of::<<#handle as #injectable>::Target>(#dep_name),
                cardinality: #cardinality::One,
                optional: false,
                dynamic: false,
                qualifier: #qualifier,
                config: true,
                resolution: #resolution_mode::Eager,
                observation: #dependency_observation::Live,
            }
        };

        let value = quote!(#resolved.ok_or(#error::MissingComponent(#dep_name))?);

        return FieldPlan {
            ready: ReadyField::same(&value),
            value,
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    // Multi-valued edges resolve every provider of a trait and never fail (empty
    // `Vec`/`HashMap` is valid), so they take no `Injectable`-missing fallback.
    if let Some(item) = attr::vec_inner(ty) {
        let dependency = dep(
            &item,
            quote!(#cardinality::Collection),
            false,
            false,
            none.clone(),
        );

        return FieldPlan {
            value: quote!(cx.dep_all::<#item>(#index).await?),
            ready: ReadyField::read(
                &binding,
                quote!(cx.dep_all_ready::<#item>(#index)),
                quote!(#binding),
            ),
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    if let Some(value) = attr::hashmap_value(ty) {
        let dependency = dep(
            &value,
            quote!(#cardinality::Keyed),
            false,
            false,
            none.clone(),
        );

        return FieldPlan {
            value: quote!(cx.dep_keyed::<#value>(#index).await?),
            ready: ReadyField::read(
                &binding,
                quote!(cx.dep_keyed_ready::<#value>(#index)),
                quote!(#binding),
            ),
            dependency: Some(dependency),
            check: None,
            wired: None,
        };
    }

    // Single edge. Peel the wrappers: `Option<…>` marks it optional, `Dynamic<…>`
    // runtime-provided; the remaining type is the `Injectable` handle to resolve.
    let (optional, after_option) = match attr::option_inner(ty) {
        Some(inner) => (true, inner),
        None => (false, ty.clone()),
    };
    let (dynamic, handle) = match attr::dynamic_inner(&after_option) {
        Some(inner) => (true, inner),
        None => (false, after_option),
    };

    let dep_name_str = match attr::arc_inner_type(&handle) {
        Ok(inner) => inner.to_token_stream().to_string(),
        Err(_) => handle.to_token_stream().to_string(),
    };
    let dep_name = LitStr::new(&dep_name_str, handle.span());

    let qualifier = match &field_qualifier {
        Some(q) => quote!(::core::option::Option::Some(#q)),
        None => none,
    };
    let wrap = |resolved: TokenStream| match (optional, dynamic) {
        (false, false) => quote!(#resolved.ok_or(#error::MissingComponent(#dep_name))?),
        (false, true) => quote!(#dynamic_ty(#resolved.ok_or(#error::MissingComponent(#dep_name))?)),
        (true, false) => resolved,
        (true, true) => quote!(#resolved.map(#dynamic_ty)),
    };
    let value = wrap(quote!(cx.dep::<#handle>(#index, #qualifier).await?));
    let ready = ReadyField::read(
        &binding,
        quote!(cx.dep_ready::<#handle>(#index, #qualifier)),
        wrap(quote!(#binding)),
    );

    let dependency = dep(
        &handle,
        quote!(#cardinality::One),
        optional,
        dynamic,
        qualifier,
    );

    // A required, concrete single edge is `Provided`-checkable; trait-object,
    // optional, and dynamic edges are not asserted in the per-macro path.
    let handle_is_trait = matches!(&handle, syn::Type::TraitObject(_))
        || matches!(attr::arc_inner_type(&handle), Ok(inner) if matches!(inner, syn::Type::TraitObject(_)));
    let target = quote!(<#handle as #injectable>::Target);

    // Eager concrete-only assert; lazy `Wired` includes trait objects too.
    let check = (!optional && !dynamic && !handle_is_trait).then(|| target.clone());
    let wired = (!optional && !dynamic).then_some(target);

    FieldPlan {
        value,
        ready,
        dependency: Some(dependency),
        check,
        wired,
    }
}
