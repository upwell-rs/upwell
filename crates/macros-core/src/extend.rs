//! The macro extension seams.
//!
//! Upwell's attribute macros are built as a **state machine** over an extension value. The
//! extension starts at [`Default`] and is driven through single-purpose phases, each mutating
//! its accumulated state; [`Expand::expand`] then reads that state. This lets `#[service]` be
//! `#[component]` + a router extension, and `#[handlers]` be `#[methods]` + an RPC extension,
//! with no forked parsers.
//!
//! The phases (each its own trait, so an extension implements only what it needs):
//!
//! 1. [`ParseKeyed`] — the macro-invocation keyed args (`#[m(key = value, ..)]`). The base arg
//!    type ([`ComponentArgs`](crate::ComponentArgs) / [`MethodArgs`](crate::MethodArgs)) parses
//!    its own common keys and hands every unknown key to the extension.
//! 2. [`ParseItem<T>`] — an optional first pass over the whole annotated item (the `ItemImpl`,
//!    or the struct/enum `DeriveInput`), to capture context (the type's name, …) or analyze it.
//! 3. [`ParseMethod`] — for impl macros, one pass per method, updating state and stripping the
//!    extension's marker attribute (e.g. `#[rpc]`).
//! 4. [`ToTokens`] — emit the accumulated contribution, appended after the base output. The
//!    earlier phases captured everything into the extension, so emission needs only `&self`,
//!    which is exactly what `ToTokens` provides. (This is also the "user-controlled control"
//!    seam: the base owns the skeleton and only splices the extension's tokens at a fixed
//!    point — the extension adds, it never rewrites.)
//!
//! [`NoExt`] is the single no-op default for every slot, so `ComponentArgs<NoExt>` is exactly
//! `#[component]` and `MethodArgs<NoExt>` is exactly `#[methods]`.

use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::ParseStream;
use syn::{Ident, ImplItemFn, Token};

use crate::client::ClientMethod;
use crate::paths::Paths;

/// Phase 1 — parse the macro-invocation keyed args, a state machine over `key = value` (or
/// bare-flag) entries. The driving arg type reads each key ident, tries its own common keys,
/// and otherwise calls [`parse_keyed`](Self::parse_keyed) with the already-read key.
///
/// [`ToTokens`] is a supertrait: it is phase 4, the emission of the accumulated state.
pub trait ParseKeyed: Default + ToTokens {
    /// Parse the value for `key` (the ident is already consumed) from `input`.
    ///
    /// - `Ok(true)` — recognized; its value (or, for a bare flag, nothing) is consumed.
    /// - `Ok(false)` — unknown; `input` is left untouched so the base errors.
    /// - `Err(..)` — recognized but malformed (spanned at the value).
    fn parse_keyed(&mut self, key: &Ident, input: ParseStream) -> syn::Result<bool> {
        let _ = (key, input);

        Ok(false)
    }

    /// The keys this extension recognizes, for the "unknown argument" diagnostic.
    fn expected_keys() -> &'static [&'static str] {
        &[]
    }
}

/// Phase 2 (optional) — a first pass over the whole annotated item `T` (an `ItemImpl`, or a
/// struct/enum `DeriveInput`), to capture context the later phases and emission need (the type
/// ident, its name, generics, the resolved crate [`Paths`], …) before per-method parsing. The
/// resolved `paths` are handed in here so an extension can store them for its `ToTokens`.
pub trait ParseItem<T>: Default {
    fn parse_item(&mut self, item: &T, paths: &Paths) -> syn::Result<()> {
        let _ = (item, paths);

        Ok(())
    }
}

/// Phase 3 (impl macros) — one pass per method. Inspect the method, update accumulated state,
/// and strip the extension's own marker attribute if present (the base has already claimed and
/// stripped `#[init]`/`#[hook]`).
///
/// Returns an optional [`ClientMethod`] hint as a byproduct: `Some(..)` makes the method part
/// of the framework's generated client (the framework owns the client emission — see
/// [`client`](crate::client)); `None` opts it out (a non-client method, or a method this
/// extension doesn't claim). The protocol fills the hint from its own signature analysis; the
/// framework assembles and emits the client.
pub trait ParseMethod: Default {
    fn parse_method(&mut self, method: &mut ImplItemFn) -> syn::Result<Option<ClientMethod>> {
        let _ = method;

        Ok(None)
    }

    /// Extra tokens the extension emits **alongside the generated client**, given the collected
    /// method hints. The framework emits them ungated, right after `#client`; the extension owns
    /// their content *and* any target gating. This is the seam a protocol uses for target- or
    /// binding-specific client codegen (the HTTP extension emits its `#[wasm_bindgen]` browser
    /// binding and its `Dto` wire-type assertions here) — so the framework core never learns about
    /// wasm or any other target. Empty by default.
    fn extra_client_tokens(
        &self,
        client_ident: &Ident,
        methods: &[ClientMethod],
        paths: &Paths,
    ) -> proc_macro2::TokenStream {
        let _ = (client_ident, methods, paths);

        proc_macro2::TokenStream::new()
    }
}

/// The base-resolved component identity handed to a struct-macro extension (via
/// [`ParseItem<ComponentContext>`]) before it emits. A component extension (e.g. the RPC
/// `Router`) needs the *base's* resolved `id`/`name` so its own output (a service descriptor,
/// a route table, …) agrees with the component — it can't re-derive them, since the user may
/// have overridden them on the base macro.
pub struct ComponentContext {
    /// The bare type ident, e.g. `Greeter`.
    pub ident: Ident,
    /// The type ident as a string literal, e.g. for `TypeDescriptor::of::<Self>(..)`.
    pub type_name: syn::LitStr,
    /// The resolved component id (the `id = ..` override, else the lowercased ident).
    pub id: syn::LitStr,
    /// The resolved display name (the `name = ..` override, else the ident).
    pub name: syn::LitStr,
    /// The scope marker paths from `scope = ..`, if specified (`None` = default singleton).
    pub scope: Option<crate::scopes::ScopeList>,
    /// The type's own `#[doc]` attributes, cloned from the item. A router extension forwards them
    /// verbatim onto its generated client(s), so `{Controller}Client` (and the wasm binding, whose
    /// docs become TypeScript JSDoc) always carries the controller's documentation.
    pub docs: Vec<syn::Attribute>,
}

/// A **component** macro extension: the struct-side analogue of an impl extension. It is a
/// [`ParseKeyed`] (its own args) + [`ParseItem<ComponentContext>`] (receives the base's
/// resolved identity) + [`ToTokens`] (emits its appended surface). The RPC `Router` — turning
/// `#[component]` into `#[service]` — is one. `ComponentArgs<NoExt>` is `#[component]`;
/// `ComponentArgs<Router>` is `#[service]`.
pub trait ComponentExt: ParseKeyed + ParseItem<ComponentContext> + ToTokens {
    /// Whether the component's field-injection factory may be overridden later (by an `#[init]`
    /// in a separate impl — as a service is), so the base defers its *eager* field-dependency
    /// assertion to that path. Default: no (assert eagerly). A `Router` (service) returns true.
    fn defers_factory(&self) -> bool {
        false
    }

    /// Whether this is a **router-class** component — a service, a controller, or any future
    /// protocol entry point. The base then forces the lazy `Wired` graph check at the
    /// component's own definition (a missing provider becomes a `cargo check` error there),
    /// rather than deferring it to an `app!` listing. Default: no. The RPC `Router` and the
    /// axum controller router return true.
    fn asserts_wired(&self) -> bool {
        false
    }
}

/// The single no-op extension: the default for every slot. `ComponentArgs<NoExt>` is exactly
/// `#[component]`; `MethodArgs<NoExt>` is exactly `#[methods]`. Emits nothing.
#[derive(Default)]
pub struct NoExt;

impl ToTokens for NoExt {
    fn to_tokens(&self, _tokens: &mut TokenStream) {}
}

impl ParseKeyed for NoExt {}
impl<T> ParseItem<T> for NoExt {}
impl ParseMethod for NoExt {}
impl ComponentExt for NoExt {}

/// Consumes the `= value` half of a `key = value` argument, or nothing for a bare flag, so a
/// [`ParseKeyed`] impl doesn't repeat the `=`-peek boilerplate.
#[inline]
pub fn eat_eq(input: ParseStream) -> syn::Result<()> {
    if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;
    }

    Ok(())
}

/// A trailing comma between entries, tolerated and consumed.
pub fn eat_comma(input: ParseStream) -> bool {
    if input.peek(Token![,]) {
        let _ = input.parse::<Token![,]>();

        true
    } else {
        false
    }
}

/// Builds the "unknown argument `{key}`, expected one of: .." error, merging the base's common
/// keys with the extension's [`expected_keys`](ParseKeyed::expected_keys).
pub fn unknown_key_error<Ext: ParseKeyed>(key: &Ident, common: &[&str]) -> syn::Error {
    let mut keys: Vec<&str> = common.to_vec();
    keys.extend_from_slice(Ext::expected_keys());

    syn::Error::new(
        key.span(),
        format!(
            "unknown argument `{key}`, expected one of: {}",
            keys.join(", ")
        ),
    )
}
