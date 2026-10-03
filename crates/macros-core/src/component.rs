//! The base component macro: `ComponentArgs<Ext>` expansion.
//!
//! Declares a system-constructed singleton component — implements `Component`, emits a
//! field-injection factory registered into the `COMPONENTS` slice, and wires providers/handle.
//! `#[component]` is `ComponentArgs<NoExt>`; `#[service]` is `ComponentArgs<Router>` — the same
//! base skeleton plus the RPC service surface the extension appends.
//!
//! The base resolves the component identity (`id`/`name`/`scope`), hands it to the extension
//! via [`ParseItem<ComponentContext>`] (so the extension's output agrees with the component),
//! and defers its eager field-DI assertion when the extension reports the factory may be
//! overridden ([`ComponentExt::defers_factory`] — as a service is, by a `#[handlers]` `#[init]`).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemStruct, LitStr};

use crate::attr::ComponentArgs;
use crate::extend::{ComponentContext, ComponentExt};
use crate::paths::Paths;
use crate::{di, handle, inject, provide};

pub fn expand<Ext: ComponentExt>(
    mut args: ComponentArgs<Ext>,
    mut item: ItemStruct,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    let self_ident = item.ident.clone();
    let providers = provide::generate_providers(&self_ident, &args, paths);
    let handle = handle::handle_impl(&self_ident, args.by_value, args.retainable, paths);
    let handle_associated_type = &handle.associated_type;
    let handle_method = &handle.method;
    let injectable = &handle.injectable;
    let provide_impl = di::provide_impl(&self_ident, paths);

    let id = args
        .id
        .clone()
        .unwrap_or_else(|| LitStr::new(&self_ident.to_string().to_lowercase(), self_ident.span()));
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| LitStr::new(&self_ident.to_string(), self_ident.span()));
    let type_name = LitStr::new(&self_ident.to_string(), self_ident.span());

    // The scope marker path: the user's `scope = <Path>` emitted verbatim (resolved in their
    // scope), or the framework's `Singleton` anchor when omitted. Raw — no rewriting — so a
    // protocol's `Request`/`Connection` (or any custom scope) is whatever the path names.
    let scope_path = args
        .scope
        .clone()
        .unwrap_or_else(|| paths.core("scope::Singleton"));
    let registrations_slice = args
        .factory_slice
        .clone()
        .unwrap_or_else(|| inject::registrations_slice_ident(&self_ident));
    let registrations_infra =
        inject::registrations_infrastructure(&self_ident, &registrations_slice, paths);

    // The type's own doc comments, forwarded to a router extension's generated client(s).
    let docs: Vec<syn::Attribute> = item
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .cloned()
        .collect();

    // Hand the resolved identity to the extension before it emits, so its output (service
    // descriptor, route table, …) names the component consistently.
    let context = ComponentContext {
        ident: self_ident.clone(),
        type_name,
        id: id.clone(),
        name: name.clone(),
        scope: args.scope.clone(),
        docs,
    };
    args.ext.parse_item(&context, paths)?;

    // An explicit `factory = path` replaces the field-injection default; so does
    // `default_factory = false` (the manual case). Otherwise the default is emitted. The
    // eager DI assertion is deferred when the extension allows a later factory override.
    let explicit = args
        .factory
        .as_ref()
        .map(|path| inject::explicit_factory(&self_ident, path, &registrations_slice, paths));
    let emit_default = explicit.is_none() && !args.no_default_factory;
    let factory = inject::field_injection_component(
        &mut item,
        &id,
        &name,
        args.ext.defers_factory(),
        &scope_path,
        &registrations_slice,
        emit_default,
        paths,
    );
    let component = paths.core("Component");

    // A router-class component (a service, a controller) forces the lazy `Wired` graph check
    // at its own definition — a missing dependency provider is an error here, not deferred to
    // an `app!` listing. A router is an app-local protocol entry point, so its providers are
    // visible at its definition.
    let assert_wired = if args.ext.asserts_wired() {
        crate::di::assert_wired(&self_ident, paths)
    } else {
        quote!()
    };

    // The extension's appended surface (nothing for `#[component]`; the service header, RPC
    // slice, and client for `#[service]`).
    let ext = &args.ext;

    // The framework-generated surface is server-only (the DI `Component` impl, field-injection
    // factory, `linkme` registration, providers, hooks). It is gated out on wasm; the extension
    // (`#ext`) — e.g. a controller's `AxumRouter` — gates itself, keeping its client struct.
    let base = crate::gate::native_only(quote! {
        #assert_wired

        impl #component for #self_ident {
            #handle_associated_type
            const ID: &'static str = #id;
            const NAME: &'static str = #name;
            #handle_method
        }

        #injectable

        #provide_impl

        #registrations_infra

        const _: () = {
            #factory

            #explicit

            #providers
        };
    });

    Ok(quote! {
        // The user's own type is kept on every target — they may legitimately use it in a wasm
        // build. Only the framework wiring above is server-gated; `dead_code` is allowed on wasm,
        // where that wiring (the sole user of some fields) is absent.
        #[cfg_attr(target_family = "wasm", allow(dead_code))]
        #item

        #base

        #ext
    })
}

#[cfg(test)]
mod tests;
