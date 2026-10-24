//! Attribute-argument parsing and small `syn` helpers shared by the macros.
//!
//! All parsing returns `syn::Result` with spanned errors so diagnostics point
//! at the offending token rather than the whole attribute.

use proc_macro2::Span;
use quote::ToTokens;
use syn::{
    GenericArgument, Generics, Ident, LitStr, PathArguments, ReturnType, Token, TraitBound, Type,
    TypeParamBound, WherePredicate,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    token,
};

use crate::extend::{NoExt, ParseKeyed, unknown_key_error};
use crate::paths::Paths;

/// The common component-macro keys, for the "unknown argument" diagnostic (the extension's
/// keys are merged in by [`unknown_key_error`]).
const COMPONENT_KEYS: &[&str] = &[
    "id",
    "name",
    "qualifier",
    "primary",
    "by_value",
    "retainable",
    "scope",
    "factory_slice",
    "factory",
    "default_factory",
    "provide",
    "priority",
    "before",
    "after",
    "upwell",
    "crate",
];

/// Parses an `upwell = <path>` / `crate = <path>` override value into a [`syn::Path`]. Shared
/// by the component and impl-block macros so the crate-root override is spelled identically.
pub(crate) fn parse_path_override(input: ParseStream) -> syn::Result<syn::Path> {
    input.parse::<Token![=]>()?;

    input.parse()
}

fn parse_scope_path(input: ParseStream) -> syn::Result<syn::Path> {
    let path: syn::Path = input.parse()?;

    if path.leading_colon.is_none()
        && path.segments.len() == 1
        && let Some(segment) = path.segments.first()
        && segment.arguments.is_empty()
    {
        return Ok(match segment.ident.to_string().as_str() {
            "singleton" => syn::parse_quote!(Singleton),
            "connection" => syn::parse_quote!(Connection),
            "request" => syn::parse_quote!(Request),
            "transient" => syn::parse_quote!(Transient),
            _ => path,
        });
    }

    Ok(path)
}

/// The base arguments of a component macro, generic over an extension `Ext` (the struct-side
/// analogue of [`MethodArgs`](crate::methods::MethodArgs)). `ComponentArgs<NoExt>` is
/// `#[component]`; `ComponentArgs<Router>` is `#[service]`. All base keys are optional:
/// - `id` / `name` — default to the type name;
/// - `provide = dyn Trait` / `provide = [dyn A, dyn B]` — traits this component provides
///   (injectable as `Arc<dyn Trait>` / `Vec<_>` / `HashMap<String, _>`); the trait must be
///   `Send + Sync` via a supertrait;
/// - `qualifier = ".."` — key for `HashMap<String, Arc<dyn Trait>>` injection;
/// - `primary` — mark this the primary provider for every trait it provides;
/// - `priority = <const i64 expression>` — order currently eligible collection providers by
///   ascending priority after enforcing `before` / `after` constraints;
/// - `before` / `after` — order every trait shared with the target component; use `as dyn Trait`
///   to require and restrict the relationship to a specific trait;
/// - `by_value` — store/inject this component as `Self` rather than `Arc<Self>`;
/// - `retainable` — with a singleton `by_value` component, explicitly asserts that cloning
///   the handle does not share generation-local mutable slots, allowing unchanged instances
///   to be retained;
/// - `scope = <ScopePath>` — the instance lifetime, named by a [`Scope`] marker type in scope
///   (e.g. `Request` from a protocol's prelude, or a custom scope); omitted means singleton;
/// - `factory_slice` / `factory` / `default_factory` — factory overrides.
///
/// Any other key is delegated to the extension's [`parse_keyed`](ParseKeyed::parse_keyed)
/// (the RPC `Router` adds `version` and `rpc_slice`, for instance).
#[derive(Default)]
pub struct ComponentArgs<Ext: ParseKeyed = NoExt> {
    pub id: Option<LitStr>,
    pub name: Option<LitStr>,
    pub provide: Vec<Type>,
    pub priority: Option<syn::Expr>,
    pub before: Vec<ProviderOrderTarget>,
    pub after: Vec<ProviderOrderTarget>,
    pub qualifier: Option<LitStr>,
    pub primary: bool,
    pub by_value: bool,
    /// Explicitly opts a by-value handle into cross-generation retention. The author
    /// guarantees that cloning the component does not share generation-local mutable slots.
    pub retainable: bool,
    /// The [`Scope`] marker-type **path** parsed from `scope = ..`, emitted as written so it
    /// resolves in the caller's scope (a protocol's `Request`/`Connection` arrives through its
    /// prelude; a custom scope works the same way). The lowercase keywords
    /// `singleton`/`connection`/`request`/`transient` are accepted as aliases for the
    /// like-named marker types. `None` means the default (singleton).
    pub scope: Option<syn::Path>,
    /// Overrides the generated per-type registration slice name (`factory_slice = Ident`).
    ///
    /// Since factories and hooks share one merged `{Type}Registrations` slice on the `linkme`
    /// backend, this name governs **both** — so a type that overrides it must set the same
    /// `factory_slice` on every one of its `#[component]` / `#[methods]` / `#[handlers]` blocks
    /// (the consistency the `#[init]` factory already required, now extended to `#[hook]`s). Left
    /// unset (the normal case) it defaults to `{Type}Registrations`.
    pub factory_slice: Option<Ident>,
    /// An explicit async factory path (`factory = path::to::fn`).
    pub factory: Option<syn::Path>,
    /// Suppresses the field-injection default factory (`default_factory = false`).
    pub no_default_factory: bool,
    /// Override for the core `upwell` facade root (`upwell = ::path`).
    pub upwell: Option<syn::Path>,
    /// Override for the plugin own-types root (`crate = ::path`).
    pub krate: Option<syn::Path>,
    /// The macro extension (its own keyed args, item pass, and emission).
    pub ext: Ext,
}

impl<Ext: ParseKeyed> ComponentArgs<Ext> {
    /// Resolves the crate [`Paths`] for this invocation: the macro's `default` roots with any
    /// `upwell =` / `crate =` overrides applied.
    pub fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), self.krate.clone())
    }
}

impl<Ext: ParseKeyed> Parse for ComponentArgs<Ext> {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ComponentArgs::<Ext>::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            match key.to_string().as_str() {
                "id" => args.id = Some(parse_value(input)?),
                "name" => args.name = Some(parse_value(input)?),
                "qualifier" => args.qualifier = Some(parse_value(input)?),
                "primary" => args.primary = true,
                "priority" => {
                    input.parse::<Token![=]>()?;
                    args.priority = Some(input.parse()?);
                }
                "by_value" => args.by_value = true,
                "retainable" => args.retainable = true,
                "factory_slice" => {
                    input.parse::<Token![=]>()?;
                    args.factory_slice = Some(input.parse()?);
                }
                "factory" => {
                    input.parse::<Token![=]>()?;
                    args.factory = Some(input.parse()?);
                }
                "default_factory" => {
                    input.parse::<Token![=]>()?;
                    let value: syn::LitBool = input.parse()?;
                    args.no_default_factory = !value.value;
                }
                "upwell" => args.upwell = Some(parse_path_override(input)?),
                "crate" => args.krate = Some(parse_path_override(input)?),
                "scope" => {
                    input.parse::<Token![=]>()?;
                    args.scope = Some(parse_scope_path(input)?);
                }
                "provide" => {
                    input.parse::<Token![=]>()?;

                    if input.peek(token::Bracket) {
                        let content;
                        syn::bracketed!(content in input);
                        let list =
                            Punctuated::<ProvidedTrait, Token![,]>::parse_terminated(&content)?;
                        args.provide.extend(list.into_iter().map(|t| t.0));
                    } else {
                        args.provide.push(input.parse::<ProvidedTrait>()?.0);
                    }
                }
                "before" => parse_order_targets(input, &mut args.before)?,
                "after" => parse_order_targets(input, &mut args.after)?,
                _ => {
                    if !args.ext.parse_keyed(&key, input)? {
                        return Err(unknown_key_error::<Ext>(&key, COMPONENT_KEYS));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        if args.by_value && !args.provide.is_empty() {
            return Err(syn::Error::new_spanned(
                &args.provide[0],
                "`by_value` cannot be combined with `provide`: trait providers share the \
                 component through an `Arc`, while a by-value component is stored as `Self`",
            ));
        }

        if args.retainable && !args.by_value {
            return Err(syn::Error::new(
                Span::call_site(),
                "`retainable` is only valid with `by_value`; Arc-backed components are already generation-safe",
            ));
        }

        if args.retainable && args.scope.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`retainable` is singleton-only and cannot be combined with an explicit `scope`",
            ));
        }

        let mut provided = std::collections::HashSet::new();

        for trait_ty in &args.provide {
            let key = trait_ty.to_token_stream().to_string();

            if !provided.insert(key) {
                return Err(syn::Error::new_spanned(
                    trait_ty,
                    "duplicate trait in `provide` declaration",
                ));
            }
        }

        if args.provide.is_empty()
            && (args.priority.is_some() || !args.before.is_empty() || !args.after.is_empty())
        {
            return Err(syn::Error::new(
                Span::call_site(),
                "`priority`, `before`, and `after` require at least one `provide` declaration",
            ));
        }

        Ok(args)
    }
}

fn parse_order_targets(
    input: ParseStream,
    targets: &mut Vec<ProviderOrderTarget>,
) -> syn::Result<()> {
    input.parse::<Token![=]>()?;
    if input.peek(token::Bracket) {
        let content;
        syn::bracketed!(content in input);
        let list = Punctuated::<ProviderOrderTarget, Token![,]>::parse_terminated(&content)?;
        if list.is_empty() {
            return Err(content.error("provider ordering target list cannot be empty"));
        }
        targets.extend(list);
    } else {
        targets.push(input.parse()?);
    }

    Ok(())
}

/// Parses the `= "value"` half of a string-valued argument.
fn parse_value(input: ParseStream) -> syn::Result<LitStr> {
    input.parse::<Token![=]>()?;

    input.parse()
}

/// One component named by a `before` or `after` provider-ordering argument.
pub struct ProviderOrderTarget {
    pub target: Type,
    pub traits: Vec<Type>,
}

impl Parse for ProviderOrderTarget {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let target: Type = input.parse()?;
        let mut traits = Vec::new();
        if matches!(target, Type::TraitObject(_)) {
            return Err(syn::Error::new_spanned(
                target,
                "provider ordering target must be a concrete component type",
            ));
        }
        if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            if input.peek(token::Bracket) {
                let content;
                syn::bracketed!(content in input);
                let list = Punctuated::<ProvidedTrait, Token![,]>::parse_terminated(&content)?;
                if list.is_empty() {
                    return Err(content.error("provider ordering trait list cannot be empty"));
                }
                traits.extend(list.into_iter().map(|provided| provided.0));
            } else {
                traits.push(input.parse::<ProvidedTrait>()?.0);
            }
        }

        Ok(Self { target, traits })
    }
}

/// A provided-trait entry parsed as a trait-object type rather than an opaque path. The trait must
/// be `Send + Sync` via supertraits.
struct ProvidedTrait(Type);

impl Parse for ProvidedTrait {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty: Type = input.parse()?;

        if !matches!(ty, Type::TraitObject(_)) {
            return Err(syn::Error::new_spanned(
                &ty,
                "`provide` expects a trait object, e.g. `dyn Repo`",
            ));
        }

        Ok(ProvidedTrait(ty))
    }
}

// The impl-block macro arguments (`#[methods(..)]` / `#[handlers(..)]`) are now the generic
// `MethodArgs<Ext>` (in `methods.rs`): the base owns `factory_slice`, and the `Rpcs` extension
// owns `rpc_slice` / `client_trait`. There is no bespoke arg type per impl macro anymore.

/// Maps the `(streamed_input, streamed_output)` pair inferred from a handler
/// signature to its `OperationKind` variant ident. The kind is structural, so
/// `#[rpc]` derives it rather than taking an annotation.
pub fn operation_ident(streamed_input: bool, streamed_output: bool) -> Ident {
    let name = match (streamed_input, streamed_output) {
        (false, false) => "Unary",
        (false, true) => "ServerStream",
        (true, false) => "ClientStream",
        (true, true) => "BidiStream",
    };

    Ident::new(name, Span::call_site())
}

/// Whether a return type's body is a `ResponseStream<T>` (after peeling an
/// optional outer `Result`), i.e. the handler streams its output.
pub fn returns_response_stream(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };

    type_name(peel_named(ty, "Result")).is_some_and(|id| id == "ResponseStream")
}

/// Whether a parameter type is the inbound-stream extractor `Streaming<T>`.
pub fn is_streaming_param(ty: &Type) -> bool {
    type_name(ty).is_some_and(|id| id == "Streaming")
}

/// Whether a parameter type is the single-body extractor `Payload<T>`.
pub fn is_payload_param(ty: &Type) -> bool {
    type_name(ty).is_some_and(|id| id == "Payload")
}

/// The logical response *body* type, for descriptor metadata only.
///
/// Handlers may return any [`Responder`](upwell_core::Responder): a bare value,
/// `Result<T, E>`, `ResponseStream<T>`, `Result<ResponseStream<T>, E>`, `()`,
/// etc. This peels the `Result` and `ResponseStream` wrappers to the body type,
/// and reports `()` for an absent return. It never fails: dispatch is uniform
/// regardless of the shape, so this only feeds the `output` field.
pub fn response_body_type(output: &ReturnType) -> Type {
    let ty = match output {
        ReturnType::Default => return syn::parse_quote!(()),
        ReturnType::Type(_, ty) => ty.as_ref(),
    };

    peel_named(peel_named(ty, "Result"), "ResponseStream").clone()
}

/// The last path-segment ident of a simple path type (e.g. `Foo` of `a::b::Foo<T>`).
/// Public so a protocol's macro crate can classify handler-parameter extractor types.
pub fn type_name(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Path(path) => path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}

/// The first type argument of `Name<T, ..>` when the last path segment is `name`.
/// Used by the client codegen to recover request/response payload types from the
/// extractor and `Responder` wrappers in a handler signature. Public so a protocol's macro
/// crate can peel its own extractor wrappers (`Path<T>`, `Json<T>`, …).
pub fn first_type_arg(ty: &Type, name: &str) -> Option<Type> {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == name
        && let PathArguments::AngleBracketed(generics) = &segment.arguments
        && let Some(GenericArgument::Type(inner)) = generics.args.first()
    {
        return Some(inner.clone());
    }

    None
}

/// The handle type `H` of an `Option<H>` field (an optional dependency).
pub fn option_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Option")
}

/// The item handle `H` of a `Vec<H>` field (a collection of all trait providers).
pub fn vec_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Vec")
}

/// The value handle `H` of a `HashMap<String, H>` field (qualifier-keyed trait
/// providers). Only matches when the key is `String`.
pub fn hashmap_value(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;

    if segment.ident != "HashMap" {
        return None;
    }

    let PathArguments::AngleBracketed(generics) = &segment.arguments else {
        return None;
    };

    let mut types = generics.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    });
    let key = types.next()?;
    let value = types.next()?;

    if type_name(key).is_some_and(|id| id == "String") {
        return Some(value.clone());
    }

    None
}

/// The handle type `H` of a `Dynamic<H>` field (a runtime-provided dependency).
pub fn dynamic_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Dynamic")
}

/// The wrapped shape of a `Lazy<H>` dependency.
pub fn lazy_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Lazy")
}

/// The wrapped shape of a `Fresh<H>` dependency.
pub fn fresh_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Fresh")
}

/// The target type of a `Deferred<T>` dependency.
pub fn deferred_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Deferred")
}

/// The config type `T` of a `Cfg<T>` field (a property-path-bound config value).
pub fn cfg_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Cfg")
}

/// The target type `T` of a live `Dep<T>` field.
pub fn dep_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Dep")
}

/// The request body type `T` of a `Payload<T>` parameter.
pub fn payload_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Payload")
}

/// The request item type `T` of a `Streaming<T>` parameter.
pub fn streaming_inner(ty: &Type) -> Option<Type> {
    first_type_arg(ty, "Streaming")
}

/// If `output` is `Result<Ok, Err?>`, returns `(Ok, Err)`. `Err` is `None` for a
/// one-argument alias such as `upwell::Result<T>` (whose error is the framework
/// `Error`, opaque to the client and surfaced as a raw body).
pub fn result_type_args(output: &ReturnType) -> Option<(Type, Option<Type>)> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };

    let Type::Path(path) = ty.as_ref() else {
        return None;
    };

    let segment = path.path.segments.last()?;

    if segment.ident != "Result" {
        return None;
    }

    let PathArguments::AngleBracketed(generics) = &segment.arguments else {
        return None;
    };

    let mut args = generics.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });

    let ok = args.next()?;
    let err = args.next();

    Some((ok, err))
}

/// If `ty` is `Name<T, ..>`, returns its first type argument `T`; otherwise `ty`.
fn peel_named<'a>(ty: &'a Type, name: &str) -> &'a Type {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == name
        && let PathArguments::AngleBracketed(generics) = &segment.arguments
        && let Some(GenericArgument::Type(inner)) = generics.args.first()
    {
        return inner;
    }

    ty
}

/// Extracts `T` from `Arc<T>` (the form `#[init]` dependency parameters take).
pub fn arc_inner_type(ty: &Type) -> syn::Result<Type> {
    let err = || {
        syn::Error::new_spanned(
            ty,
            "#[init] parameters must be `Arc<Component>` dependencies",
        )
    };

    let Type::Path(path) = ty else {
        return Err(err());
    };

    let segment = path.path.segments.last().ok_or_else(err)?;

    if segment.ident != "Arc" {
        return Err(err());
    }

    let PathArguments::AngleBracketed(generics) = &segment.arguments else {
        return Err(err());
    };

    match generics.args.first() {
        Some(GenericArgument::Type(inner)) => Ok(inner.clone()),
        _ => Err(err()),
    }
}

/// The item shape of a detected `Stream`, recovered from an `impl Stream<Item =
/// ..>` type or a generic parameter bound by `Stream`.
///
/// `error` is `Some` when the item is `Result<T, E>` (a per-item-fallible
/// stream): `item` is then the success type `T` and `error` the `E`. For a bare
/// `Item = T` stream `error` is `None`.
pub struct StreamShape {
    pub item: Type,
    pub error: Option<Type>,
}

impl StreamShape {
    /// Whether each item is a `Result` the framework maps to a `StreamError`.
    pub fn fallible(&self) -> bool {
        self.error.is_some()
    }
}

/// Detects whether `ty` is a `Stream` the macro can introspect — either
/// `impl Stream<Item = ..>` or a generic parameter of `generics` bound by
/// `Stream` — and recovers its [`StreamShape`]. A concrete named type is opaque
/// here (the macro cannot prove it is a `Stream`), so it returns `None`; such
/// returns are flagged explicitly with `#[rpc(stream)]`.
pub fn stream_shape(ty: &Type, generics: &Generics) -> Option<StreamShape> {
    let item = match ty {
        Type::ImplTrait(it) => stream_item_in_bounds(&it.bounds)?,

        Type::Path(path) if path.qself.is_none() => {
            let name = path.path.get_ident()?;

            generic_stream_item(name, generics)?
        }

        _ => return None,
    };

    Some(shape_from_item(item))
}

/// Splits a stream's `Item` type into its success/error parts: `Result<T, E>`
/// becomes a fallible shape, anything else a bare-value shape.
fn shape_from_item(item: Type) -> StreamShape {
    match result_args_of_type(&item) {
        Some((ok, err)) => StreamShape {
            item: ok,
            error: Some(err),
        },

        None => StreamShape { item, error: None },
    }
}

/// Finds the `Item = X` binding of a `Stream` bound among `bounds`, if present.
fn stream_item_in_bounds(bounds: &Punctuated<TypeParamBound, Token![+]>) -> Option<Type> {
    bounds.iter().find_map(|bound| match bound {
        TypeParamBound::Trait(trait_bound) => stream_item_of_bound(trait_bound),
        _ => None,
    })
}

/// The `Item = X` of a single `Stream<Item = X>` trait bound, or `None` if the
/// bound is not `Stream` or names no `Item`.
fn stream_item_of_bound(bound: &TraitBound) -> Option<Type> {
    let segment = bound.path.segments.last()?;

    if segment.ident != "Stream" {
        return None;
    }

    let PathArguments::AngleBracketed(generics) = &segment.arguments else {
        return None;
    };

    generics.args.iter().find_map(|arg| match arg {
        GenericArgument::AssocType(assoc) if assoc.ident == "Item" => Some(assoc.ty.clone()),
        _ => None,
    })
}

/// The `Item` of a generic parameter `name` bound by `Stream`, checking both the
/// inline bounds (`<S: Stream<..>>`) and the `where` clause.
fn generic_stream_item(name: &Ident, generics: &Generics) -> Option<Type> {
    let inline = generics.params.iter().find_map(|param| match param {
        syn::GenericParam::Type(type_param) if &type_param.ident == name => {
            stream_item_in_bounds(&type_param.bounds)
        }
        _ => None,
    });

    if inline.is_some() {
        return inline;
    }

    let where_clause = generics.where_clause.as_ref()?;

    where_clause
        .predicates
        .iter()
        .find_map(|predicate| match predicate {
            WherePredicate::Type(predicate) if type_name(&predicate.bounded_ty) == Some(name) => {
                stream_item_in_bounds(&predicate.bounds)
            }
            _ => None,
        })
}

/// If `ty` is `Result<Ok, Err>`, returns `(Ok, Err)`. Used to split a stream
/// item into success and error halves.
fn result_args_of_type(ty: &Type) -> Option<(Type, Type)> {
    let Type::Path(path) = ty else {
        return None;
    };

    let segment = path.path.segments.last()?;

    if segment.ident != "Result" {
        return None;
    }

    let PathArguments::AngleBracketed(generics) = &segment.arguments else {
        return None;
    };

    let mut args = generics.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });

    let ok = args.next()?;
    let err = args.next()?;

    Some((ok, err))
}

/// Whether a return type is a `Result<...>` (vs. an infallible bare value).
pub fn returns_result(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };

    let Type::Path(path) = ty.as_ref() else {
        return false;
    };

    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Result")
}

#[cfg(test)]
mod tests;
