//! Shared `provide = ..` codegen, used by `#[component]` and `#[service]`.
//!
//! For each trait a component provides, emits a `ProviderDescriptor` registered
//! into the `PROVIDERS` slice, plus an `erase` fn that re-types the *already
//! built* `Arc<Concrete>` as `Arc<dyn Trait + Send + Sync>`. The erase is a plain
//! `Arc::clone` + unsizing coercion — the same single instance, aliased under the
//! trait's key — never a second construction.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::{Ident, LitStr};

use crate::extend::ParseKeyed;
use crate::{attr::ComponentArgs, paths::Paths};

/// Emits the provider registrations for `self_ident` given the parsed args.
/// Empty when the component provides nothing.
pub fn generate_providers<Ext: ParseKeyed>(
    self_ident: &Ident,
    args: &ComponentArgs<Ext>,
    paths: &Paths,
) -> TokenStream {
    if args.provide.is_empty() {
        return quote!();
    }

    let self_name = LitStr::new(&self_ident.to_string(), self_ident.span());
    let concrete_ty = crate::source::declared_type(self_ident, self_ident, &self_name, paths);
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let provider_descriptor = paths.core("ProviderDescriptor");
    let providers_slice = paths.core("PROVIDERS");
    let runtime_descriptor = paths.core("RuntimeDescriptor");
    let component_descriptor = paths.core("ComponentDescriptor");
    let type_descriptor = paths.core("TypeDescriptor");
    let provider_of = paths.core("ProviderOf");
    let erase_provider = paths.core("erase_provider");
    let provider_live_cell = paths.core("provider_live_cell");
    let provider_order = paths.core("ProviderOrder");
    let provider_order_direction = paths.core("ProviderOrderDirection");
    let descriptor = paths.core("Descriptor");

    // Qualifier defaults to the component's id (explicit `id`, else lowercased
    // type name), overridable with `qualifier = ".."`.
    let qualifier = args
        .qualifier
        .clone()
        .or_else(|| args.id.clone())
        .unwrap_or_else(|| LitStr::new(&self_ident.to_string().to_lowercase(), self_ident.span()));
    let primary = args.primary;
    let priority = args
        .priority
        .as_ref()
        .map_or_else(|| quote!(0i64), |priority| quote!(#priority));
    let order = args
        .before
        .iter()
        .map(|target| (target, quote!(#provider_order_direction::Before)))
        .chain(
            args.after
                .iter()
                .map(|target| (target, quote!(#provider_order_direction::After))),
        )
        .map(|(target, direction)| {
            let concrete = &target.target;
            let concrete_name =
                LitStr::new(&concrete.to_token_stream().to_string(), Span::call_site());
            let traits = target.traits.iter().map(|trait_ty| {
                let trait_name =
                    LitStr::new(&trait_ty.to_token_stream().to_string(), Span::call_site());
                quote!(#type_descriptor::of::<#trait_ty>(#trait_name))
            });
            quote! {
                #provider_order {
                    target: #type_descriptor::of::<#concrete>(#concrete_name),
                    traits: &[#(#traits),*],
                    direction: #direction,
                }
            }
        });
    let ordering = quote!(&[#(#order),*]);
    let provided_traits = &args.provide;
    let mut seen_ordering_assertions = std::collections::HashSet::new();
    let mut ordering_assertions = Vec::new();

    for target in args.before.iter().chain(args.after.iter()) {
        let concrete = &target.target;

        for trait_ty in &target.traits {
            let key = format!(
                "{} as {}",
                concrete.to_token_stream(),
                trait_ty.to_token_stream()
            );

            if seen_ordering_assertions.insert(key) {
                ordering_assertions.push(quote! {
                    __upwell_assert_ordering_provider::<#concrete, #trait_ty>();
                });
            }
        }
    }

    let assertions = quote! {
        const _: () = {
            fn __upwell_assert_injectable<T>()
            where
                T: ?Sized + #runtime_descriptor<#component_descriptor>,
            {
            }

            fn __upwell_assert_ordering_provider<T, P: ?Sized>()
            where
                T: #provider_of<P>,
            {
            }

            fn __upwell_check_provider_contracts() {
                #(__upwell_assert_injectable::<#provided_traits>();)*
                #(#ordering_assertions)*
            }
        };
    };

    // Per-trait helper items (`ProviderOf` upcast impls), the
    // descriptor literals aggregated into the by-type const, and the linkme
    // statics that index back into it. A component provides many traits, so the
    // by-type handle is one-to-many: a single `Descriptor<&[ProviderDescriptor]>`
    // impl per type — not one `Descriptor<ProviderDescriptor>` impl per trait,
    // which would collide (`E0119`).
    let mut helpers = Vec::with_capacity(args.provide.len());
    let mut descriptor_literals = Vec::with_capacity(args.provide.len());
    let mut statics = Vec::with_capacity(args.provide.len());

    for (i, dyn_ty) in args.provide.iter().enumerate() {
        // The trait object as written (`dyn Trait`). Both this provide side and
        // the dependency side spell it identically, so their keys match. The
        // trait must be `Send + Sync` (via supertraits, since components are
        // shared across threads); otherwise the `erase_provider::<Self, dyn Trait>`
        // bound fails to compile, pointing the author at the missing bound.
        let trait_name = LitStr::new(&dyn_ty.to_token_stream().to_string(), Span::call_site());
        let provider_ident = format_ident!("__UPWELL_PROVIDER_{}", i);

        helpers.push(quote! {
            impl #provider_of<#dyn_ty> for #self_ident {
                fn upcast(
                    this: ::std::sync::Arc<Self>,
                ) -> ::std::sync::Arc<#dyn_ty> {
                    this
                }
            }
        });

        descriptor_literals.push(quote! {
            #provider_descriptor {
                trait_ty: #type_descriptor::of::<#dyn_ty>(#trait_name),
                concrete_ty: #concrete_ty,
                qualifier: #qualifier,
                primary: #primary,
                priority: #priority,
                ordering: #ordering,
                erase: #erase_provider::<#self_ident, #dyn_ty>,
                live: #provider_live_cell::<#dyn_ty>,
            }
        });

        // Index back into the by-type const so the auto-discovery slice and the
        // explicit `builder.provider::<T>()` path share one source of truth.
        statics.push(quote! {
            #[#distributed_slice(#providers_slice)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static #provider_ident: #provider_descriptor =
                <#self_ident as #descriptor<&'static [#provider_descriptor]>>::DESCRIPTOR[#i];
        });
    }

    quote! {
        #assertions
        #(#helpers)*

        impl #descriptor<&'static [#provider_descriptor]> for #self_ident {
            const DESCRIPTOR: &'static [#provider_descriptor] = &[
                #(#descriptor_literals),*
            ];
        }

        #(#statics)*
    }
}
