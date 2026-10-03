//! Shared codegen for a component's storage handle (`Component::Handle` +
//! `into_handle`), used by `#[component]` and `#[service]`.
//!
//! The default wraps the component in `Arc<Self>` (auto-wrapped, the common
//! case). `#[component(by_value)]` instead stores it as `Self` and emits the
//! self-`Injectable` impl that makes `Self` a valid handle — for types that
//! manage their own sharing (typically internally `Arc`, cheap to clone).

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::paths::Paths;

/// The pieces a `Component` impl needs for its handle: the associated type and
/// `into_handle` method to splice around the constants, plus an optional
/// standalone `Injectable` impl (non-empty only for `by_value`).
pub struct HandleImpl {
    pub associated_type: TokenStream,
    pub method: TokenStream,
    pub injectable: TokenStream,
}

pub fn handle_impl(self_ident: &Ident, by_value: bool, paths: &Paths) -> HandleImpl {
    if by_value {
        let injectable = paths.core("Injectable");

        HandleImpl {
            associated_type: quote! {
                type Handle = Self;
            },
            method: quote! {
                fn into_handle(self) -> Self::Handle {
                    self
                }
            },
            injectable: quote! {
                impl #injectable for #self_ident {
                    type Target = #self_ident;
                    type Stored = Self;

                    fn into_stored(self) -> Self {
                        self
                    }

                    fn from_stored(stored: &Self) -> Self {
                        ::core::clone::Clone::clone(stored)
                    }

                }
            },
        }
    } else {
        HandleImpl {
            associated_type: quote! {
                type Handle = ::std::sync::Arc<Self>;
            },
            method: quote! {
                fn into_handle(self) -> Self::Handle {
                    ::std::sync::Arc::new(self)
                }
            },
            injectable: quote!(),
        }
    }
}
