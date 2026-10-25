//! Shared codegen library for the Upwell proc-macros.
//!
//! A proc-macro crate can only export proc-macros, so the reusable codegen lives here as an
//! ordinary library that the macro crates build on:
//!
//! - **Core macros** — `#[component]`, `#[config]`, `#[methods]`, `#[injectable]` — are
//!   expanded here and surfaced by [`upwell-macros`] as thin shims.
//! - **Building blocks** — attribute parsing ([`attr`]), the extension seams ([`extend`]),
//!   crate-path resolution ([`paths`]), field-injection ([`inject`]), hooks ([`hook`]), the
//!   DI assertions ([`di`]), provider wiring ([`provide`]), the handle helper ([`handle`]),
//!   descriptor source positions ([`source`]),
//!   and the base impl-macro state machine ([`methods`]) — are **public**, so a plugin's macro
//!   crate (e.g. `upwell-rpc-macros`) reuses them to build its own macros (`#[service]`,
//!   `#[handlers]`, …) without forking the codegen.
//!
//! [`upwell-macros`]: https://docs.rs/upwell-macros

pub mod attr;
pub mod backend;
pub mod client;
pub mod di;
pub mod entry;
pub mod extend;
pub mod gate;
pub mod handle;
pub mod hook;
pub mod inject;
pub mod methods;
pub mod paths;
pub mod provide;
pub mod source;

mod app;
mod case;
mod component;
mod config;
mod injectable;
mod when;

pub use client::{Capability, ClientMethod};
pub use extend::{
    ComponentContext, ComponentExt, NoExt, ParseItem, ParseKeyed, ParseMethod, eat_comma, eat_eq,
    unknown_key_error,
};
pub use paths::Paths;

/// The generic component-macro expansion, reused by a plugin's component-variant macro (e.g.
/// `#[service]` = `ComponentArgs<Router>`).
pub use component::expand as expand_component;

use proc_macro2::TokenStream;
use syn::{DeriveInput, ItemImpl, ItemStruct, ItemTrait};

/// `app!` expansion entry point for a named protocol-agnostic application definition.
pub fn app(input: TokenStream) -> TokenStream {
    run::<app::NamedApp, _>(input, |input| Ok(app::expand(input)))
}

/// Parses `item` as `T` and runs `expand`, turning a parse or expansion error into a
/// `compile_error!` token stream so the macro never panics. Exposed so plugin macro crates
/// reuse the same parse-and-expand harness.
pub fn run<T, F>(item: TokenStream, expand: F) -> TokenStream
where
    T: syn::parse::Parse,
    F: FnOnce(T) -> syn::Result<TokenStream>,
{
    match syn::parse2::<T>(item) {
        Ok(parsed) => expand(parsed).unwrap_or_else(syn::Error::into_compile_error),

        Err(e) => e.into_compile_error(),
    }
}

/// `#[component]` expansion entry point — the base component macro with no extension.
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match syn::parse2::<attr::ComponentArgs>(attr) {
        Ok(args) => args,

        Err(e) => return e.into_compile_error(),
    };

    let paths = args.paths(Paths::upwell());

    run::<ItemStruct, _>(item, |item| component::expand(args, item, &paths))
}

/// `#[config]` expansion entry point.
pub fn config(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match syn::parse2::<config::ConfigArgs>(attr) {
        Ok(args) => args,

        Err(e) => return e.into_compile_error(),
    };

    let paths = args.paths(Paths::upwell());

    run::<DeriveInput, _>(item, |item| config::expand(args, item, &paths))
}

/// `#[methods]` expansion entry point — the base impl macro with no extension.
pub fn methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match syn::parse2::<methods::MethodArgs>(attr) {
        Ok(args) => args,

        Err(e) => return e.into_compile_error(),
    };

    let paths = args.paths(Paths::upwell());

    run::<ItemImpl, _>(item, |item| methods::expand(args, item, &paths))
}

/// `#[when]` expansion entry point: passes through an attribute a component macro already
/// lowered, and reports a placement error otherwise.
pub fn when(attr: TokenStream, item: TokenStream) -> TokenStream {
    when::expand_attr(attr, item)
}

/// `#[injectable]` expansion entry point.
pub fn injectable(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match syn::parse2::<injectable::InjectableArgs>(attr) {
        Ok(args) => args,

        Err(e) => return e.into_compile_error(),
    };
    let paths = args.paths(Paths::upwell());

    run::<ItemTrait, _>(item, |item| Ok(injectable::expand(item, &paths)))
}
