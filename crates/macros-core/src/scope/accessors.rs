//! The generated reads of the active instance of a `#[scope]` type.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Fields;

use crate::paths::Paths;

/// `S::is_active()` for every scope and, for a scope with fields, `S::current()`: reads of the
/// active invocation's instance of the scope.
pub(super) fn accessors(fields: &Fields, ident: &syn::Ident, paths: &Paths) -> TokenStream {
    let is_active = paths.core("context::scope_is_active");
    let scope_value = paths.core("context::scope_value");
    let error = paths.core("context::ScopedError");
    let current = if fields.is_empty() {
        quote!()
    } else {
        quote! {
            /// This scope's value in the active invocation.
            ///
            /// # Errors
            ///
            /// When the active invocation runs in no instance of this scope.
            pub fn current() -> ::core::result::Result<::std::sync::Arc<Self>, #error> {
                #scope_value::<Self>()
            }
        }
    };

    quote! {
        impl #ident {
            /// Whether the active invocation runs in an instance of this scope.
            pub fn is_active() -> bool {
                #is_active::<Self>()
            }

            #current
        }
    }
}
