//! Optional compile-time dependency-injection checks, emitted only under the
//! `di-check` feature.
//!
//! Each component registers itself with `impl Provided<Self> for Wiring`, and each
//! field-injected component asserts its concrete dependencies with an uncalled
//! `fn _() where Wiring: Provided<Dep> { }` — whose bound the compiler discharges
//! at definition, so a missing provider is a `cargo check` error. Trait-object
//! (`Arc<dyn Trait>`) dependencies are intentionally not asserted here: the
//! "single provider" rule needs a whole-graph view (see the `app!` design); the
//! build-time source analyzer covers them instead.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::paths::Paths;

/// Whether compile-time DI checking is enabled for this build.
pub fn enabled() -> bool {
    cfg!(feature = "di-check")
}

/// `impl Provided<Self> for Wiring {}`, registering a component as a provider of
/// its own concrete type. Empty unless `di-check` is on.
pub fn provide_impl(self_ident: &Ident, paths: &Paths) -> TokenStream {
    if !enabled() {
        return quote!();
    }

    let provided = paths.core("Provided");
    let wiring = paths.core("Wiring");

    quote! {
        impl #provided<#self_ident> for #wiring {}
    }
}

/// `impl Wired for T where Wiring: Provided<T1> + .. {}` — a lazy predicate that
/// all of `T`'s single dependencies (`targets`, concrete and trait-object) are
/// provided, checked only where `app!` demands `T: Wired`. A type with no
/// dependencies is unconditionally `Wired`. Empty unless `di-check` is on.
pub fn wired_impl(self_ident: &Ident, targets: &[TokenStream], paths: &Paths) -> TokenStream {
    if !enabled() {
        return quote!();
    }

    let wired = paths.core("Wired");
    let provided = paths.core("Provided");
    let wiring = paths.core("Wiring");

    if targets.is_empty() {
        quote! {
            impl #wired for #self_ident {}
        }
    } else {
        quote! {
            impl #wired for #self_ident
            where
                #wiring: #(#provided<#targets>)+*,
            {
            }
        }
    }
}

/// `impl Provided<dyn Trait> for Wiring {}`, emitted once at the trait's
/// definition by `#[injectable]`, so a single `Arc<dyn Trait>` dependency
/// type-checks. Living on the trait (not each provider) means it is coherent no
/// matter how many components `provide` it — provider *existence* is checked at
/// runtime and by the source analyzer. Empty unless `di-check` is on.
pub fn injectable_impl(trait_ident: &Ident, paths: &Paths) -> TokenStream {
    if !enabled() {
        return quote!();
    }

    let provided = paths.core("Provided");
    let wiring = paths.core("Wiring");

    quote! {
        impl #provided<dyn #trait_ident> for #wiring {}
    }
}

/// Forces the lazy [`Wired`](wired_impl) predicate for `self_ident` at this point: an uncalled
/// `fn _() where Self: Wired {}`, so the whole-graph dependency check (including trait-object
/// providers) is discharged at the type's own definition rather than deferred to an `app!`
/// listing. Empty unless `di-check` is on.
///
/// Suited to an app-local router type (a `#[controller]`) whose dependencies are provided in
/// the same binary; the providers' `Provided` impls are visible at the definition.
pub fn assert_wired(self_ident: &Ident, paths: &Paths) -> TokenStream {
    if !enabled() {
        return quote!();
    }

    let wired = paths.core("Wired");

    quote! {
        const _: () = {
            fn __upwell_assert_wired<T: #wired>() {}

            fn __upwell_check() {
                __upwell_assert_wired::<#self_ident>();
            }
        };
    }
}

/// An uncalled assertion that every `target` is provided, as the bound
/// `Wiring: Provided<T1> + Provided<T2> + ..`. Each `target` is a `Provided` type
/// argument (e.g. `<Arc<T> as Injectable>::Target`). Empty unless `di-check` is
/// on or there are no targets.
pub fn assert(targets: &[TokenStream], paths: &Paths) -> TokenStream {
    if !enabled() || targets.is_empty() {
        return quote!();
    }

    let provided = paths.core("Provided");
    let wiring = paths.core("Wiring");

    quote! {
        const _: () = {
            fn __upwell_assert_di()
            where
                #wiring: #(#provided<#targets>)+*,
            {
            }
        };
    }
}
