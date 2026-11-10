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
use crate::request_seed::{RequestSeeder, SeederMiddleware, read_seed};

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
    /// parent. They run inside the protocol's body limit and request timeout, and only for
    /// requests a route matches (they are applied with `Router::route_layer`), so a request no
    /// route matches takes the plain 404 path without reaching them. The first one registered
    /// is the outermost. The request scope reads
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
    /// holding the `ScopeOpener` of its session scope, and wraps it via [`as_layer`]. Like
    /// every parent layer, it runs only for requests a route matches.
    fn parent_middleware<M>(self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>;

    /// Registers the [`RequestSeeder`] `M`, a DI singleton, to seed its `T` into the
    /// [`HttpRequest`](crate::HttpRequest) scope of every request a controller route serves.
    ///
    /// The seeder runs inside the parent layers and before global middleware and the eager
    /// request-scope opening, so every opening of a controller route's scope holds the seed. A
    /// rejection ends the request with the seeder's response. WebSocket upgrades and requests
    /// no route matches skip it, and open their scope, when something injects from it, without
    /// the seed. Preparation therefore accepts a requirement of `T` only from what controller
    /// routes alone reach: a handler and the components it injects. An eager request-scoped
    /// component, one no controller route reaches, and one a DI-backed middleware
    /// ([`middleware`](Self::middleware), [`parent_middleware`](Self::parent_middleware))
    /// reaches over a declared edge, such as a `Current` accessor, must not require it, and are
    /// rejected with `OpenerWithoutSeed` naming the non-controller requests. Middleware that
    /// injects at run time without a declared edge, such as a raw `from_fn` layer calling
    /// `Inject`, is not seen: such an injection fails when it resolves on a request without the
    /// seed. Seeders run in registration order. Registering `M` again is a
    /// no-op; a second seeder of the same `T` fails preparation with
    /// [`DuplicateRequestSeeder`](crate::Error::DuplicateRequestSeeder).
    fn request_seed<T, M>(self) -> Self
    where
        T: Clone + Send + Sync + 'static,
        M: RequestSeeder<Seed = T> + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>;

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
        self.protocol_mut()
            .middleware
            .push(raw_applier(layer, Attach::Router));

        self
    }

    fn middleware<M>(mut self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut()
            .middleware
            .push(di_applier::<M>(Attach::Router));
        self.protocol_mut()
            .middleware_roots
            .push(upwell_core::TypeDescriptor::of::<M>(M::NAME));

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
        self.protocol_mut()
            .parent_layers
            .push(raw_applier(layer, Attach::MatchedRoutes));

        self
    }

    fn parent_middleware<M>(mut self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut()
            .parent_layers
            .push(di_applier::<M>(Attach::MatchedRoutes));
        self.protocol_mut()
            .middleware_roots
            .push(upwell_core::TypeDescriptor::of::<M>(M::NAME));

        self.component::<M>()
    }

    fn request_seed<T, M>(mut self) -> Self
    where
        T: Clone + Send + Sync + 'static,
        M: RequestSeeder<Seed = T> + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        let seeds = &mut self.protocol_mut().request_seeds;

        if !seeds.register::<T, M>() {
            return self;
        }

        seeds.layers.push(Box::new(|runtime, router| {
            let seeder = runtime
                .root()
                .get::<M>()
                .expect("request seeder missing from DI root — did you register it?");

            Attach::MatchedRoutes.apply(router, as_layer(Arc::new(SeederMiddleware(seeder))))
        }));
        seeds.readers.push(read_seed::<T>);

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
        let claims = P::seed_claims(&options);
        let rejected =
            P::check_options(&options)
                .err()
                .map(|source| crate::Error::WebsocketBuild {
                    protocol: std::any::type_name::<P>(),
                    source: Box::new(source),
                });

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
            claims,
            rejected,
        });

        self
    }
}

/// Which requests an applier's layer wraps.
#[derive(Clone, Copy)]
enum Attach {
    /// Every request the router serves, the fallback included (`Router::layer`).
    Router,
    /// Only requests a route matches (`Router::route_layer`), so an unmatched request takes
    /// the plain fallback.
    MatchedRoutes,
}

impl Attach {
    fn apply<L>(self, router: axum::Router, layer: L) -> axum::Router
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        match self {
            Self::Router => router.layer(layer),
            Self::MatchedRoutes if router.has_routes() => router.route_layer(layer),
            Self::MatchedRoutes => router,
        }
    }
}

/// Applies the raw `layer` to the requests `attach` names.
fn raw_applier<L>(layer: L, attach: Attach) -> MiddlewareApplier
where
    L: Layer<Route> + Clone + Send + Sync + 'static,
    L::Service: Service<Request> + Clone + Send + Sync + 'static,
    <L::Service as Service<Request>>::Response: IntoResponse + 'static,
    <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
    <L::Service as Service<Request>>::Future: Send + 'static,
{
    Box::new(move |_runtime, router| attach.apply(router, layer))
}

/// Resolves the DI singleton `M` once the runtime is built and applies it to the requests
/// `attach` names.
fn di_applier<M>(attach: Attach) -> MiddlewareApplier
where
    M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
{
    Box::new(move |runtime, router| {
        let mw = runtime
            .root()
            .get::<M>()
            .expect("middleware component missing from DI root — did you register it?");

        attach.apply(router, as_layer(mw))
    })
}
