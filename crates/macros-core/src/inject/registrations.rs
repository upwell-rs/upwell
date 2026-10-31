//! The per-type registration plumbing every component and recipe macro shares: the merged
//! `{Type}Registrations` slice, its accessor impls, and appending a factory to it.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};

use crate::paths::Paths;

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

/// Appends `factory`, a `ComponentFactoryDescriptor` literal, to `self_ty`'s registrations:
/// an `inventory` submission, or a `static` named `name` in its `linkme` slice.
pub fn register_factory(
    self_ty: &impl ToTokens,
    registrations_slice: &syn::Ident,
    name: &syn::Ident,
    factory: &TokenStream,
    paths: &Paths,
) -> TokenStream {
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let registration = paths.core("Registration");
    let descriptor_for = paths.core("DescriptorFor");

    crate::backend::dual_backend(
        quote! {
            #inventory::submit! {
                #descriptor_for::<#self_ty, #component_factory_descriptor>::new(#factory)
            }
        },
        quote! {
            #[#distributed_slice(#registrations_slice)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static #name: #registration = #registration::Factory(#factory);
        },
    )
}
