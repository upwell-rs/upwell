//! Shared field-injection factory generation, used by `#[service]` (as the
//! overridable default) and `#[component]` (as the component's factory).
//!
//! Emits a singleton factory that constructs the struct field by field. Every
//! field is a **dependency** resolved from the container, unless it carries
//! `#[default]`, which makes it local owned state built via `Default::default()`.
//! A dependency field's type is its [`Injectable`] *handle*; the wrapper around
//! that handle selects the edge shape:
//! - `H` — a required single dependency (`Arc<T>`, or a by-value `Injectable`);
//! - `Option<H>` — an optional dependency (resolves to `None` if absent);
//! - `Dynamic<H>` — a runtime-provided dependency (exempt from static validation);
//! - `Option<Dynamic<H>>` — both.
//!
//! Each dependency is keyed under `<H as Injectable>::Target`, so a blanket
//! `Arc<T>` keys by `T` while a by-value handle keys by itself.
//!
//! [`Injectable`]: upwell_core::Injectable

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Expr, ExprLit, Field, Fields, ItemStruct, Lit, LitStr, Meta, spanned::Spanned};

use crate::{attr, paths::Paths};

/// The per-type registration slice identifier, `{Type}Registrations` — the merged `linkme` slice
/// holding both factory and hook entries (as `Registration`). Declared by `#[component]` /
/// `#[service]`; each `#[init]` / `factory = ..` / `#[hook]` appends to it. One slice (one linker
/// section) per component instead of two. Unused on the `inventory` backend, which keys separate
/// `DescriptorFor<Type, _>` buckets by owner type + descriptor kind.
pub fn registrations_slice_ident(self_ident: &syn::Ident) -> syn::Ident {
    format_ident!("{}Registrations", self_ident)
}

/// Declares the per-type registration infrastructure: the `ComponentFactories` / `ComponentHooks`
/// accessor impls (returning `&'static [T]`), plus either the merged `{Type}Registrations` `linkme`
/// slice or the two `inventory` collections — chosen by the active backend
/// ([`backend::dual_backend`](crate::backend::dual_backend)).
///
/// Neither backend can hand back a borrowed typed slice directly (the `linkme` slice is a mixed
/// `[Registration]` enum; the `inventory` collection is a linked list), so each accessor materializes
/// its kind's entries into a per-type `OnceLock<Vec<_>>` cache — built once at app build, when the
/// registry reads these — and returns `.as_slice()`, preserving the `&'static [T]` contract.
pub fn registrations_infrastructure(
    self_ident: &syn::Ident,
    slice: &syn::Ident,
    paths: &Paths,
) -> TokenStream {
    let component_factories = paths.core("ComponentFactories");
    let component_hooks = paths.core("ComponentHooks");
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let hook_descriptor = paths.core("HookDescriptor");
    let registration = paths.core("Registration");
    let descriptor_for = paths.core("DescriptorFor");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");

    let factory_registry = crate::backend::registry_for_impl(
        quote!(#self_ident),
        quote!(#component_factory_descriptor),
        paths,
    );
    let hook_registry =
        crate::backend::registry_for_impl(quote!(#self_ident), quote!(#hook_descriptor), paths);

    let inventory_tokens = quote! {
        #factory_registry
        #hook_registry

        impl #component_factories for #self_ident {
            fn factories() -> &'static [#component_factory_descriptor] {
                static CACHE: ::std::sync::OnceLock<::std::vec::Vec<#component_factory_descriptor>> =
                    ::std::sync::OnceLock::new();

                CACHE
                    .get_or_init(|| {
                        #inventory::iter::<#descriptor_for<#self_ident, #component_factory_descriptor>>
                            .into_iter()
                            .map(|__entry| **__entry)
                            .collect()
                    })
                    .as_slice()
            }
        }

        impl #component_hooks for #self_ident {
            fn hooks() -> &'static [#hook_descriptor] {
                static CACHE: ::std::sync::OnceLock<::std::vec::Vec<#hook_descriptor>> =
                    ::std::sync::OnceLock::new();

                CACHE
                    .get_or_init(|| {
                        let mut __hooks: ::std::vec::Vec<#hook_descriptor> =
                            #inventory::iter::<#descriptor_for<#self_ident, #hook_descriptor>>
                                .into_iter()
                                .map(|__entry| **__entry)
                                .collect();
                        __hooks.sort_by_key(|__hook| __hook.ordinal);
                        __hooks
                    })
                    .as_slice()
            }
        }
    };

    let linkme_tokens = quote! {
        #[#distributed_slice]
        #[linkme(crate = #linkme_crate)]
        #[allow(unsafe_code)]
        #[allow(non_upper_case_globals)]
        pub static #slice: [#registration];

        impl #component_factories for #self_ident {
            fn factories() -> &'static [#component_factory_descriptor] {
                static CACHE: ::std::sync::OnceLock<::std::vec::Vec<#component_factory_descriptor>> =
                    ::std::sync::OnceLock::new();

                CACHE
                    .get_or_init(|| {
                        #slice
                            .iter()
                            .filter_map(#registration::as_factory)
                            .copied()
                            .collect()
                    })
                    .as_slice()
            }
        }

        impl #component_hooks for #self_ident {
            fn hooks() -> &'static [#hook_descriptor] {
                static CACHE: ::std::sync::OnceLock<::std::vec::Vec<#hook_descriptor>> =
                    ::std::sync::OnceLock::new();

                CACHE
                    .get_or_init(|| {
                        let mut __hooks: ::std::vec::Vec<#hook_descriptor> = #slice
                            .iter()
                            .filter_map(#registration::as_hook)
                            .copied()
                            .collect();
                        __hooks.sort_by_key(|__hook| __hook.ordinal);
                        __hooks
                    })
                    .as_slice()
            }
        }
    };

    crate::backend::dual_backend(inventory_tokens, linkme_tokens)
}

#[allow(clippy::too_many_arguments)]
pub fn field_injection_component(
    item: &mut ItemStruct,
    id: &LitStr,
    name: &LitStr,
    defer_di_assert: bool,
    scope_path: &syn::Path,
    registrations_slice: &syn::Ident,
    emit_default_factory: bool,
    paths: &Paths,
) -> TokenStream {
    let self_ident = item.ident.clone();
    let boxed_component = paths.core("BoxedComponent");
    let component_construction_context = paths.core("ComponentConstructionContext");
    let component_descriptor = paths.core("ComponentDescriptor");
    let component_factories = paths.core("ComponentFactories");
    let component_hooks = paths.core("ComponentHooks");
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let components_slice = paths.core("COMPONENTS");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let component = paths.core("Component");
    let injectable = paths.core("Injectable");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let registration = paths.core("Registration");
    let descriptor_for = paths.core("DescriptorFor");
    let result = paths.core("DiResult");
    let type_descriptor = paths.core("TypeDescriptor");
    let descriptor_trait = paths.core("Descriptor");

    let mut inits = Vec::new();
    let mut dep_descriptors = Vec::new();
    let mut checks = Vec::new();
    let mut wired_targets = Vec::new();

    let mut plan = |inits: &mut Vec<TokenStream>, prefix: TokenStream, field: &mut Field| {
        let FieldPlan {
            value,
            dependency,
            check,
            wired,
        } = plan_field(field, paths);

        inits.push(quote!(#prefix #value));

        if let Some(dep) = dependency {
            dep_descriptors.push(dep);
        }

        if let Some(check) = check {
            checks.push(check);
        }

        if let Some(wired) = wired {
            wired_targets.push(wired);
        }
    };

    let construct = match &mut item.fields {
        Fields::Named(named) => {
            for field in &mut named.named {
                let field_ident = field.ident.clone().expect("named field");
                plan(&mut inits, quote!(#field_ident:), field);
            }

            quote!(#self_ident { #(#inits),* })
        }

        Fields::Unnamed(unnamed) => {
            for field in &mut unnamed.unnamed {
                plan(&mut inits, quote!(), field);
            }

            quote!(#self_ident( #(#inits),* ))
        }

        Fields::Unit => quote!(#self_ident),
    };

    // The field-injection default factory — its dep assertions, construction fn,
    // and slice entry. Suppressed for a manual component (`default_factory = false`),
    // which is provided as an instance rather than built.
    let default_factory = if emit_default_factory {
        // Assert deps eagerly only when the field-injection factory is the real one.
        // A `#[service]` (and any type whose construction an `#[init]` may override)
        // defers to the `#[init]` path and the source analyzer, so its field deps are
        // not necessarily the real ones.
        let di_assert = if defer_di_assert {
            quote!()
        } else {
            crate::di::assert(&checks, paths)
        };

        // The lazy `Wired` predicate — every single dep, incl. trait objects —
        // checked when `app!` demands `T: Wired`.
        let wired = crate::di::wired_impl(&self_ident, &wired_targets, paths);

        let factory_literal = quote! {
            #component_factory_descriptor {
                id: "default",
                construct: __upwell_factory,
                dependencies: __upwell_deps,
                default: true,
            }
        };
        let register_default = crate::backend::dual_backend(
            quote! {
                #inventory::submit! {
                    #descriptor_for::<#self_ident, #component_factory_descriptor>::new(#factory_literal)
                }
            },
            quote! {
                #[#distributed_slice(#registrations_slice)]
                #[linkme(crate = #linkme_crate)]
                #[allow(unsafe_code)]
                static __UPWELL_DEFAULT_FACTORY: #registration =
                    #registration::Factory(#factory_literal);
            },
        );

        quote! {
            #di_assert

            #wired

            #[allow(unused_variables)]
            fn __upwell_factory(
                cx: &mut #component_construction_context,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                        Output = #result<#boxed_component>,
                    > + ::core::marker::Send + '_,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    let __instance = #construct;

                    ::core::result::Result::Ok(#boxed_component {
                        ty: #type_descriptor::of::<#self_ident>(#name),
                        value: ::std::boxed::Box::new(
                            #injectable::into_stored(
                                <#self_ident as #component>::into_handle(__instance),
                            ),
                        ),
                    })
                })
            }

            fn __upwell_deps() -> ::std::vec::Vec<#dependency_descriptor> {
                ::std::vec![ #(#dep_descriptors),* ]
            }

            // The field-injection default, appended to the type's registrations. Used
            // only when no explicit (`#[init]` / `factory = ..`) factory is present.
            #register_default
        }
    } else {
        // A manual component still strips field attrs (done above) but builds
        // nothing. It is trivially `Wired` — it has no constructed dependencies.
        let _ = (&construct, &dep_descriptors, &checks, &wired_targets);

        crate::di::wired_impl(&self_ident, &[], paths)
    };

    quote! {
        #default_factory

        const __UPWELL_COMPONENT_DESCRIPTOR: #component_descriptor =
            #component_descriptor {
                id: #id,
                name: #name,
                ty: #type_descriptor::of::<#self_ident>(#name),
                scope: &#scope_path,
                condition: ::core::option::Option::None,
                factories: <#self_ident as #component_factories>::factories,
                hooks: <#self_ident as #component_hooks>::hooks,
                generation_snapshot: #component_descriptor::typed_snapshot::<#self_ident>(),
            };

        impl #descriptor_trait<#component_descriptor> for #self_ident {
            const DESCRIPTOR: #component_descriptor = __UPWELL_COMPONENT_DESCRIPTOR;
        }

        #[#distributed_slice(#components_slice)]
        #[linkme(crate = #linkme_crate)]
        #[allow(unsafe_code)]
        static __UPWELL_COMPONENT: #component_descriptor = __UPWELL_COMPONENT_DESCRIPTOR;
    }
}

/// Emits an explicit `factory = path` factory entry: it appends a non-default
/// [`ComponentFactoryDescriptor`] to the type's factory slice, driving the given
/// async factory through the build-time `Factory` machinery (so the path's
/// signature need not be visible — its deps come from its parameters' `FromContainer`
/// impls). Overrides the field-injection default.
pub fn explicit_factory(
    self_ident: &syn::Ident,
    factory_path: &syn::Path,
    registrations_slice: &syn::Ident,
    paths: &Paths,
) -> TokenStream {
    let component_construction_context = paths.core("ComponentConstructionContext");
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let dispatch_factory = paths.core("dispatch_factory");
    let factory_dependencies = paths.core("factory_dependencies");
    let boxed_component = paths.core("BoxedComponent");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let registration = paths.core("Registration");
    let descriptor_for = paths.core("DescriptorFor");
    let result = paths.core("DiResult");

    let factory_literal = quote! {
        #component_factory_descriptor {
            id: concat!("factory:", stringify!(#factory_path)),
            construct: __upwell_explicit_factory,
            dependencies: __upwell_explicit_deps,
            default: false,
        }
    };
    let register = crate::backend::dual_backend(
        quote! {
            #inventory::submit! {
                #descriptor_for::<#self_ident, #component_factory_descriptor>::new(#factory_literal)
            }
        },
        quote! {
            #[#distributed_slice(#registrations_slice)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static __UPWELL_EXPLICIT_FACTORY: #registration =
                #registration::Factory(#factory_literal);
        },
    );

    quote! {
        fn __upwell_explicit_deps() -> ::std::vec::Vec<#dependency_descriptor> {
            #factory_dependencies(#factory_path)
        }

        #[allow(unused_variables)]
        fn __upwell_explicit_factory(
            cx: &mut #component_construction_context,
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<
                    Output = #result<#boxed_component>,
                > + ::core::marker::Send + '_,
            >,
        > {
            #dispatch_factory(#factory_path, cx)
        }

        #register
    }
}

/// One field's contribution: the value expression that builds it in the struct
/// literal, and the dependency descriptor it registers (absent for `#[default]`
/// local state).
struct FieldPlan {
    value: TokenStream,
    dependency: Option<TokenStream>,
    /// The `Provide<Target>` type to assert eagerly for this field, when it is a
    /// required *concrete* dependency (trait-object / optional / dynamic / multi
    /// edges are not eagerly asserted in the per-macro path).
    check: Option<TokenStream>,
    /// The `Provide<Target>` type for the lazy `Wired` bound — every required
    /// single dependency, *including* trait objects (checked only via `app!`).
    wired: Option<TokenStream>,
}

/// Classifies a field and, as a side effect, strips the `#[default]` marker so
/// the emitted struct stays valid.
fn plan_field(field: &mut Field, paths: &Paths) -> FieldPlan {
    let had_default = field.attrs.iter().any(|a| a.path().is_ident("default"));
    field.attrs.retain(|a| !a.path().is_ident("default"));

    if had_default {
        return FieldPlan {
            value: quote!(::core::default::Default::default()),
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

        return FieldPlan {
            value: quote!(cx.fresh::<#handle>(#qualifier)),
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

        return FieldPlan {
            value: quote!(cx.deferred::<#target>(#qualifier)?),
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
        let value = match (&field_qualifier, attr::arc_inner_type(&handle)) {
            (Some(qualifier), Ok(target)) => {
                quote!(cx.lazy_qualified::<#target>(#qualifier))
            }
            _ => quote!(<#ty as #from_container>::from_container(cx).await?),
        };

        return FieldPlan {
            value,
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

        return FieldPlan {
            value: quote!(#resolved.ok_or(#error::MissingComponent(#dep_name))?),
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
            value: quote!(cx.resolve_all::<#item>().await?),
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
            value: quote!(cx.resolve_keyed::<#value>().await?),
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

    let (resolved, qualifier) = match &field_qualifier {
        Some(q) => (
            quote!(cx.resolve_qualified::<#handle>(#q).await?),
            quote!(::core::option::Option::Some(#q)),
        ),
        None => (quote!(cx.resolve::<#handle>().await?), none),
    };
    let value = match (optional, dynamic) {
        (false, false) => quote!(#resolved.ok_or(#error::MissingComponent(#dep_name))?),
        (false, true) => quote!(#dynamic_ty(#resolved.ok_or(#error::MissingComponent(#dep_name))?)),
        (true, false) => resolved,
        (true, true) => quote!(#resolved.map(#dynamic_ty)),
    };

    let dependency = dep(
        &handle,
        quote!(#cardinality::One),
        optional,
        dynamic,
        qualifier,
    );

    // A required, concrete single edge is `Provide`-checkable; trait-object,
    // optional, and dynamic edges are not asserted in the per-macro path.
    let handle_is_trait = matches!(&handle, syn::Type::TraitObject(_))
        || matches!(attr::arc_inner_type(&handle), Ok(inner) if matches!(inner, syn::Type::TraitObject(_)));
    let target = quote!(<#handle as #injectable>::Target);

    // Eager concrete-only assert; lazy `Wired` includes trait objects too.
    let check = (!optional && !dynamic && !handle_is_trait).then(|| target.clone());
    let wired = (!optional && !dynamic).then_some(target);

    FieldPlan {
        value,
        dependency: Some(dependency),
        check,
        wired,
    }
}
