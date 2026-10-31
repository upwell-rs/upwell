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
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Field, Fields, ItemStruct, LitStr};

use crate::paths::Paths;

mod field;
mod registrations;

use field::{FieldPlan, ReadyField, plan_field};
pub use registrations::{
    register_factory, registrations_infrastructure, registrations_slice_ident,
};

#[allow(clippy::too_many_arguments)]
pub fn field_injection_component(
    item: &mut ItemStruct,
    id: &LitStr,
    name: &LitStr,
    defer_di_assert: bool,
    scope_path: &syn::Path,
    registrations_slice: &syn::Ident,
    emit_default_factory: bool,
    condition: &TokenStream,
    paths: &Paths,
) -> TokenStream {
    let self_ident = item.ident.clone();
    let boxed_component = paths.core("BoxedComponent");
    let component_construction_context = paths.core("ComponentConstructionContext");
    let component_descriptor = paths.core("ComponentDescriptor");
    let generation_snapshot = paths.core("GenerationSnapshot");
    let scope_trait = paths.core("Scope");
    let component_factories = paths.core("ComponentFactories");
    let component_hooks = paths.core("ComponentHooks");
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let component_factory_role = paths.core("ComponentFactoryRole");
    let components_slice = paths.core("COMPONENTS");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let component = paths.core("Component");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let result = paths.core("DiResult");
    let type_descriptor = paths.core("TypeDescriptor");
    let descriptor_trait = paths.core("Descriptor");

    let mut inits = Vec::new();
    let mut ready_inits = Vec::new();
    let mut ready_reads = Vec::new();
    let mut dep_descriptors = Vec::new();
    let mut checks = Vec::new();
    let mut wired_targets = Vec::new();

    let mut plan = |inits: &mut Vec<TokenStream>,
                    ready_inits: &mut Vec<TokenStream>,
                    prefix: TokenStream,
                    field: &mut Field| {
        let FieldPlan {
            value,
            ready,
            dependency,
            check,
            wired,
        } = plan_field(field, dep_descriptors.len(), paths);

        inits.push(quote!(#prefix #value));

        let ReadyField { read, value } = ready;

        ready_inits.push(quote!(#prefix #value));
        ready_reads.extend(read);

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

    let (construct, construct_ready) = match &mut item.fields {
        Fields::Named(named) => {
            for field in &mut named.named {
                let field_ident = field.ident.clone().expect("named field");
                plan(&mut inits, &mut ready_inits, quote!(#field_ident:), field);
            }

            (
                quote!(#self_ident { #(#inits),* }),
                quote!(#self_ident { #(#ready_inits),* }),
            )
        }

        Fields::Unnamed(unnamed) => {
            for field in &mut unnamed.unnamed {
                plan(&mut inits, &mut ready_inits, quote!(), field);
            }

            (
                quote!(#self_ident( #(#inits),* )),
                quote!(#self_ident( #(#ready_inits),* )),
            )
        }

        Fields::Unit => (quote!(#self_ident), quote!(#self_ident)),
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
                construct_ready: ::core::option::Option::Some(__upwell_ready_factory),
                dependencies: __upwell_deps,
                role: #component_factory_role::Default,
            }
        };
        let register_default = register_factory(
            &self_ident,
            registrations_slice,
            &format_ident!("__UPWELL_DEFAULT_FACTORY"),
            &factory_literal,
            paths,
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

                    ::core::result::Result::Ok(__upwell_boxed(cx, __instance))
                })
            }

            #[allow(unused_variables)]
            fn __upwell_ready_factory(
                cx: &#component_construction_context,
            ) -> #result<::core::option::Option<#boxed_component>> {
                #(#ready_reads)*

                let __instance = #construct_ready;

                ::core::result::Result::Ok(::core::option::Option::Some(__upwell_boxed(cx, __instance)))
            }

            fn __upwell_boxed(
                cx: &#component_construction_context,
                instance: #self_ident,
            ) -> #boxed_component {
                #boxed_component {
                    ty: #type_descriptor::of::<#self_ident>(#name),
                    value: ::std::boxed::Box::new(
                        cx.stored(<#self_ident as #component>::into_handle(instance)),
                    ),
                }
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
        let _ = (
            &construct,
            &construct_ready,
            &ready_reads,
            &dep_descriptors,
            &checks,
            &wired_targets,
        );

        crate::di::wired_impl(&self_ident, &[], paths)
    };

    let declared_ty = crate::source::declared_type(&self_ident, &self_ident, &name, paths);
    let scope_key = quote_spanned! {scope_path.span()=> <#scope_path as #scope_trait>::KEY };

    quote! {
        #default_factory

        const __UPWELL_COMPONENT_DESCRIPTOR: #component_descriptor =
            #component_descriptor {
                id: #id,
                name: #name,
                ty: #declared_ty,
                scope: #scope_key,
                condition: #condition,
                factories: <#self_ident as #component_factories>::factories,
                hooks: <#self_ident as #component_hooks>::hooks,
                generation_snapshot: ::core::option::Option::Some(#generation_snapshot::of::<#self_ident>()),
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

/// A non-default `ComponentFactoryDescriptor` literal with id `id` that drives the async
/// `factory` through the build-time `Factory` machinery, as `factories!` expands an entry:
/// the factory's parameters supply both construction and the dependency edges, so its
/// signature need not be visible to the macro.
///
/// `ready` is a synchronous function taking the same parameters, which builds the component
/// without a future when every parameter is ready to read.
pub fn factory_descriptor(
    id: &TokenStream,
    factory: &TokenStream,
    ready: Option<&TokenStream>,
    paths: &Paths,
) -> TokenStream {
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let component_factory_role = paths.core("ComponentFactoryRole");
    let dispatch_factory = paths.core("dispatch_factory");
    let dispatch_ready_factory = paths.core("dispatch_ready_factory");
    let factory_dependencies = paths.core("factory_dependencies");

    let construct_ready = match ready {
        Some(ready) => quote! {
            ::core::option::Option::Some(|cx| #dispatch_ready_factory(#ready, cx))
        },
        None => quote!(::core::option::Option::None),
    };

    quote! {
        #component_factory_descriptor {
            id: #id,
            construct: |cx| #dispatch_factory(#factory, cx),
            construct_ready: #construct_ready,
            dependencies: || #factory_dependencies(#factory),
            role: #component_factory_role::Explicit,
        }
    }
}

/// Emits an explicit `factory = path` factory entry, appended to the type's factory slice;
/// it overrides the field-injection default.
pub fn explicit_factory(
    self_ident: &syn::Ident,
    factory_path: &syn::Path,
    registrations_slice: &syn::Ident,
    paths: &Paths,
) -> TokenStream {
    let factory = factory_descriptor(
        &quote!(concat!("factory:", stringify!(#factory_path))),
        &quote!(#factory_path),
        None,
        paths,
    );

    register_factory(
        self_ident,
        registrations_slice,
        &format_ident!("__UPWELL_EXPLICIT_FACTORY"),
        &factory,
        paths,
    )
}
