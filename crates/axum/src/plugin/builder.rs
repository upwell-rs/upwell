//! The axum-specific methods of [`AppBuilder<Axum>`].

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::Route;
use tower::{Layer, Service};
use upwell_app::AppBuilder;
#[cfg(feature = "ws")]
use upwell_app::AppRuntime;
use upwell_core::Descriptor;
use upwell_di::{Component, ComponentDescriptor};

use super::Axum;
#[cfg(feature = "ws")]
use super::WsRegistration;
use crate::controller::ControllerDescriptor;
use crate::middleware::{AxumMiddleware, MiddlewareApplier, as_layer};

/// axum-specific builder methods, contributed to [`AppBuilder<Axum>`] as an extension
/// trait. Bring it into scope to register controllers by type; it is in the prelude.
///
/// Controllers also auto-register through the [`CONTROLLERS`] slice, so an
/// `auto_discover`d app needs no explicit registration — this is the explicit path (and
/// the one the `app!` macro could drive).
pub trait AxumAppBuilder {
    /// Registers controller type `T`: its route header and its construction factory.
    fn controller<T>(self) -> Self
    where
        T: Descriptor<ControllerDescriptor> + Descriptor<ComponentDescriptor>;

    /// Manually registers a raw controller header (prefer [`controller`](Self::controller)).
    fn controller_descriptor(self, descriptor: &'static ControllerDescriptor) -> Self;

    /// Wraps the whole app in a raw `tower::Layer` — any standard axum/tower middleware
    /// (`tower-http`, a hand-written `axum::middleware::from_fn`, …) works here unmodified,
    /// with the same bound as [`axum::Router::layer`] itself. The first layer registered is
    /// the outermost; interleaves in registration order with [`middleware`](Self::middleware).
    fn layer<L>(self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static;

    /// Sugar over [`layer`](Self::layer): resolves `M` as a DI singleton (shared across every
    /// attach point it's registered at, instead of constructed per attach point) and wraps it
    /// via [`as_layer`].
    fn middleware<M>(self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>;

    /// Wraps the app in a raw `tower::Layer` that picks the scope each request's
    /// [`HttpRequest`](crate::HttpRequest) scope opens under, by inserting a
    /// [`RequestParent`](crate::RequestParent) into the request's extensions; a request
    /// without one opens under the application root.
    ///
    /// Parent layers run before every [`layer`](Self::layer) and
    /// [`middleware`](Self::middleware), and before the request scope opens for eager
    /// request-scoped components and `Current`/`Scoped` accessors, so every opening sees the
    /// parent. They run inside the protocol's body limit and request timeout, for routed
    /// requests only; the first one registered is the outermost. The request scope reads
    /// [`RequestParent`](crate::RequestParent) only when at least one parent layer is
    /// registered, so an application without one pays nothing.
    fn parent_layer<L>(self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static;

    /// Sugar over [`parent_layer`](Self::parent_layer), as [`middleware`](Self::middleware)
    /// is over [`layer`](Self::layer): resolves `M` as a DI singleton, such as a session cache
    /// holding the `ScopeOpener` of its session scope, and wraps it via [`as_layer`].
    fn parent_middleware<M>(self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>;

    /// Opts the app into a WebSocket protocol `P`, mounting its upgrade handler at `path` with the
    /// protocol's default [`Options`](crate::ws::WebsocketProtocol::Options). Only
    /// `#[controller(ws = P)]` controllers speaking `P` are then served, under `path` (the path
    /// can't be inferred, so it is given here). The same protocol may be mounted more than once
    /// with distinct paths and options. Duplicate paths are rejected during preparation.
    #[cfg(feature = "ws")]
    fn register_ws<P>(self, path: impl Into<String>) -> Self
    where
        P: crate::ws::WebsocketProtocol,
        P::Options: Default;

    /// Like [`register_ws`](Self::register_ws), but with explicit per-endpoint
    /// [`Options`](crate::ws::WebsocketProtocol::Options) — e.g. a `StompConfig` selecting the STOMP
    /// heart-beat interval and accepted versions.
    #[cfg(feature = "ws")]
    fn register_ws_with<P>(self, path: impl Into<String>, options: P::Options) -> Self
    where
        P: crate::ws::WebsocketProtocol;
}

impl AxumAppBuilder for AppBuilder<Axum> {
    fn controller<T>(mut self) -> Self
    where
        T: Descriptor<ControllerDescriptor> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut()
            .controllers
            .push(<T as Descriptor<ControllerDescriptor>>::DESCRIPTOR);

        self.component::<T>()
    }

    fn controller_descriptor(mut self, descriptor: &'static ControllerDescriptor) -> Self {
        self.protocol_mut().controllers.push(*descriptor);

        self
    }

    fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.protocol_mut().middleware.push(raw_applier(layer));

        self
    }

    fn middleware<M>(mut self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut().middleware.push(di_applier::<M>());

        self.component::<M>()
    }

    fn parent_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.protocol_mut().parent_layers.push(raw_applier(layer));

        self
    }

    fn parent_middleware<M>(mut self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut().parent_layers.push(di_applier::<M>());

        self.component::<M>()
    }

    #[cfg(feature = "ws")]
    #[track_caller]
    fn register_ws<P>(self, path: impl Into<String>) -> Self
    where
        P: crate::ws::WebsocketProtocol,
        P::Options: Default,
    {
        self.register_ws_with::<P>(path, P::Options::default())
    }

    #[cfg(feature = "ws")]
    #[track_caller]
    fn register_ws_with<P>(mut self, path: impl Into<String>, options: P::Options) -> Self
    where
        P: crate::ws::WebsocketProtocol,
    {
        let path = path.into();

        // Capture the path and options in the mount closure so the non-generic registration can
        // carry protocol-specific `Options` without erasing their type.
        let mount_path = path.clone();
        let mount = Box::new(move |controllers, runtime: &AppRuntime| {
            crate::ws::mount_ws::<P>(&mount_path, controllers, runtime, options)
        });

        self.protocol_mut().ws_registrations.push(WsRegistration {
            path,
            #[cfg(feature = "tooling")]
            source: upwell_core::DescriptorSource::caller(),
            protocol: std::any::TypeId::of::<P>(),
            protocol_name: std::any::type_name::<P>(),
            mount,
            register: P::register,
        });

        self
    }
}

/// Applies the raw `layer` to the router being built.
fn raw_applier<L>(layer: L) -> MiddlewareApplier
where
    L: Layer<Route> + Clone + Send + Sync + 'static,
    L::Service: Service<Request> + Clone + Send + Sync + 'static,
    <L::Service as Service<Request>>::Response: IntoResponse + 'static,
    <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
    <L::Service as Service<Request>>::Future: Send + 'static,
{
    Box::new(move |_runtime, router| router.layer(layer))
}

/// Resolves the DI singleton `M` once the runtime is built and applies it to the router.
fn di_applier<M>() -> MiddlewareApplier
where
    M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
{
    Box::new(|runtime, router| {
        let mw = runtime
            .root()
            .get::<M>()
            .expect("middleware component missing from DI root — did you register it?");

        router.layer(as_layer(mw))
    })
}
