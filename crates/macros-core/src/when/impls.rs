//! The `ConditionInput` impls that let a `#[when]` expression use a type as a receiver.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, LitStr};

use crate::paths::Paths;

/// A component reads as its `Eligibility` in the candidate
/// generation.
pub(crate) fn component_input(ident: &Ident, paths: &Paths) -> TokenStream {
    let condition_input = paths.core("ConditionInput");
    let context = paths.core("ConditionContext");
    let eligibility = paths.core("Eligibility");
    let input_id = paths.core("ConditionInputId");
    let component = paths.core("Component");

    quote! {
        impl #condition_input for #ident {
            type Value<'a> = #eligibility;

            const INPUT: #input_id = #input_id::Component(<Self as #component>::ID);

            fn read<'a>(context: #context<'a>) -> ::core::option::Option<#eligibility> {
                context
                    .eligible(<Self as #component>::ID)
                    .map(#eligibility)
            }
        }
    }
}

/// A `#[config(path = ..)]` type reads as its staged value at its declared binding path.
pub(crate) fn config_input(ident: &Ident, path: &LitStr, paths: &Paths) -> TokenStream {
    let condition_input = paths.core("ConditionInput");
    let context = paths.core("ConditionContext");
    let input_id = paths.core("ConditionInputId");
    let binding_id = paths.core("ConfigBindingId");

    quote! {
        impl #condition_input for #ident {
            type Value<'a> = &'a Self;

            const INPUT: #input_id = #input_id::Binding(#binding_id::of::<Self>(#path));

            fn read<'a>(context: #context<'a>) -> ::core::option::Option<&'a Self> {
                context.binding::<Self>(#path)
            }
        }
    }
}
