//! The field plan of a `#[config]` / `#[config("path")]` field: a config binding injected as
//! `Cfg<T>`, keyed by property path.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::spanned::Spanned;
use syn::{LitStr, Type};

use super::field::{FieldPlan, ReadyField};
use crate::attr;
use crate::paths::Paths;

/// The plan of the config field of type `ty`, bound to `path` or to the sole binding of its
/// type.
///
/// Config lives outside the container in a `ConfigStore` resolver, reached through the
/// construction context's resolver set; `get_resolver` is called by UFCS so the generated code
/// needs no `use` of the extension trait. Config edges are validated against the registered
/// bindings, not the component graph, so the plan emits no di-check assertion.
pub(super) fn plan(ty: &Type, path: Option<&LitStr>, paths: &Paths) -> FieldPlan {
    let cardinality = paths.core("Cardinality");
    let error = paths.core("DiError");
    let injectable = paths.core("Injectable");
    let type_descriptor = paths.core("TypeDescriptor");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let resolution_mode = paths.core("ResolutionMode");
    let dependency_observation = paths.core("DependencyObservation");
    let dep_name_str = match attr::cfg_inner(ty) {
        Some(inner) => inner.to_token_stream().to_string(),
        None => ty.to_token_stream().to_string(),
    };
    let dep_name = LitStr::new(&dep_name_str, ty.span());
    let (resolved, qualifier) = resolution(ty, path, paths);

    let dependency = quote! {
        #dependency_descriptor {
            name: #dep_name,
            ty: #type_descriptor::of::<<#ty as #injectable>::Target>(#dep_name),
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

    FieldPlan {
        ready: ReadyField::same(&value),
        value,
        dependency: Some(dependency),
        check: None,
        wired: None,
    }
}

/// The expression resolving the binding of `ty` and the edge's qualifier: the property path
/// when there is one.
fn resolution(ty: &Type, path: Option<&LitStr>, paths: &Paths) -> (TokenStream, TokenStream) {
    let config_store = paths.core("ConfigStore");
    let resolver_ctx_ext = paths.core("ResolverCtxExt");
    let store = quote!(#resolver_ctx_ext::get_resolver::<#config_store>(cx));

    match path {
        Some(path) => (
            quote!(#store.and_then(|store| store.resolve_path::<#ty>(#path))),
            quote!(::core::option::Option::Some(#path)),
        ),
        None => (
            quote!(#store.and_then(|store| store.resolve_sole::<#ty>())),
            quote!(::core::option::Option::None),
        ),
    }
}
