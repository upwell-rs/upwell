//! Controllers: the HTTP-facing services, and how they register their routes.
//!
//! A controller is a DI singleton (like an RPC service) annotated with `#[controller]`. Its
//! `#[handlers]` impl blocks contribute routes. The macro emits a [`ControllerDescriptor`]
//! into the [`CONTROLLERS`] slice and an implementation of [`Controller`]; the
//! [`Axum`](crate::Axum) folds the slice on `auto_discover` and merges each
//! controller's [`axum::Router`] when the protocol is built.

use std::sync::Arc;

use upwell_app::AppRuntime;
use upwell_core::{ItemSource, TypeDescriptor, UpwellDescriptor};

/// One `#[handlers]` block's route-group builder, tagged with its controller type `C`.
///
/// The route registries hold bare `fn` pointers, which cannot implement [`UpwellDescriptor`]
/// (a primitive fn-pointer type is foreign and carries no local type, so the marker impl would
/// violate the orphan rule). This local newtype is the HTTP analog of the RPC `RpcGroup`: it wraps
/// the builder so it *can* be an `UpwellDescriptor` and thus a `DescriptorFor<C, ControllerRoute<C>>`
/// bucket element on the `inventory` backend. `Copy` is manual (a naive derive would wrongly demand
/// `C: Copy`); the wrapped fn pointer is always `Copy`.
pub struct ControllerRoute<C> {
    /// Builds this handlers block's router.
    pub build: fn(Arc<C>, &AppRuntime) -> axum::Router,
    /// Static routes retained without constructing runtime state.
    pub routes: &'static [HttpRouteDescriptor],
}

/// Static HTTP method/path/handler metadata retained during preparation.
#[derive(Clone, Copy, Debug)]
pub struct HttpRouteDescriptor {
    /// Rust handler method name.
    pub handler: &'static str,
    /// Uppercase HTTP method.
    pub method: &'static str,
    /// Controller-relative path.
    pub path: &'static str,
    /// Named placeholders parsed from the route template.
    pub path_parameters: &'static [HttpPathParameterDescriptor],
    /// Ordered semantic handler inputs.
    pub inputs: &'static [HttpInputDescriptor],
    /// Semantic response payload and transport shape.
    pub output: HttpOutputDescriptor,
    /// The handler method's declaration, when the registering macro recorded it.
    pub source: Option<ItemSource>,
}

/// One named placeholder in an HTTP route template.
#[derive(Clone, Copy, Debug)]
pub struct HttpPathParameterDescriptor {
    /// Placeholder name without the catch-all marker.
    pub name: &'static str,
    /// Whether this placeholder captures the remaining path.
    pub catch_all: bool,
}

/// The transport or server-side source of one HTTP handler input.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum HttpInputSource {
    Path,
    Query,
    Header,
    JsonBody,
    FormBody,
    BytesBody,
    RawFormBody,
    MultipartBody,
    Injected,
    Stream,
    Context,
}

/// One ordered semantic HTTP handler input.
#[derive(Clone, Copy, Debug)]
pub struct HttpInputDescriptor {
    /// Handler parameter name where one is available.
    pub name: &'static str,
    /// Semantic input source.
    pub source: HttpInputSource,
    /// Extracted value type rather than the outer extractor wrapper.
    pub ty: TypeDescriptor,
}

/// The response transport shape of one HTTP handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HttpOutputShape {
    Unary,
    NdjsonStream,
    RawStream,
    CustomStream,
    Opaque,
}

/// Semantic output metadata for one HTTP handler.
#[derive(Clone, Copy, Debug)]
pub struct HttpOutputDescriptor {
    /// Decoded payload or stream-item type when statically knowable.
    pub ty: Option<TypeDescriptor>,
    /// Original declared return type for opaque/custom diagnostics.
    pub declared: &'static str,
    /// Response transport shape.
    pub shape: HttpOutputShape,
    /// Known status-specific response alternatives.
    pub responses: &'static [HttpResponseDescriptor],
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum HttpResponseBodyDescriptor {
    Empty,
    Typed(TypeDescriptor),
    Opaque,
}

/// One known status-specific response alternative.
#[derive(Clone, Copy, Debug)]
pub struct HttpResponseDescriptor {
    /// Concrete HTTP status.
    pub status: u16,
    /// Decoded body type when declared or cheaply inferred.
    pub body: HttpResponseBodyDescriptor,
    /// Literal redirect target when cheaply inferred or explicitly declared.
    pub redirect: Option<&'static str>,
}

impl<C> Clone for ControllerRoute<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for ControllerRoute<C> {}

impl<C: 'static> UpwellDescriptor for ControllerRoute<C> {}

/// A controller's link-time registration: its identity and a builder for its routes.
///
/// The `router` builder is handed the assembled [`AppRuntime`] so it can resolve the
/// controller singleton once (capturing it in the route closures) before returning the
/// fully-pathed [`axum::Router`]. It is a plain `fn` pointer so it can live in a
/// `#[distributed_slice]`.
#[derive(Clone, Copy)]
pub struct ControllerDescriptor {
    /// The controller's id (defaults to the lowercased type name).
    pub id: &'static str,

    /// The controller's display name (the type name).
    pub name: &'static str,

    /// The controller's concrete type.
    pub ty: TypeDescriptor,

    /// The base path every route in this controller is mounted under.
    pub base: &'static str,

    /// Builds this controller's routes, with full paths already joined onto [`base`](Self::base).
    pub router: fn(&AppRuntime) -> axum::Router,
    /// Returns every static HTTP route declaration for this controller.
    pub routes: fn() -> Vec<HttpRouteDescriptor>,
}

/// The link-time slice every `#[controller]` registers into, mirroring the RPC `SERVICES`
/// slice. [`Axum::auto_discover`](crate::Axum) folds it into the builder.
#[linkme::distributed_slice]
pub static CONTROLLERS: [ControllerDescriptor];

/// Implemented by every `#[controller]` struct: the base path and a builder for its routes.
///
/// Generated alongside the [`ControllerDescriptor`]; both point at the same `router`
/// builder, so registering a controller by type and discovering it from the slice are
/// equivalent.
pub trait Controller {
    /// The base path this controller's routes are mounted under (e.g. `"/users"`).
    const BASE: &'static str;

    /// Builds this controller's [`axum::Router`], resolving the controller singleton from
    /// the runtime and capturing it in the route handlers.
    fn router(runtime: &AppRuntime) -> axum::Router;

    /// Returns static HTTP route groups without constructing runtime state.
    fn routes() -> Vec<ControllerRoute<Self>>
    where
        Self: Sized;
}
