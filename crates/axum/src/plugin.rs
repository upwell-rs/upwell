//! The axum protocol definition and its builder extension.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::Route;
use tower::{Layer, Service};
use upwell_app::{
    AppBuilder, AppRegistry, AppRuntime, PreparedProtocol, ProtocolDefinition, ValidationContext,
};
use upwell_config::{ConfigBinding, ContainerConfigExt};
use upwell_core::{Descriptor, TypeDescriptor};
use upwell_di::{BoxedComponent, Component, ComponentDescriptor};

use crate::config::{AXUM_CONFIG_PATH, AxumConfig};
use crate::controller::{CONTROLLERS, ControllerDescriptor};
use crate::extract::ScopeHandle;
use crate::middleware::{AxumMiddleware, MiddlewareApplier, as_layer};
use crate::protocol::AxumRuntime;
use crate::request_meta::{REQUEST_META_DESCRIPTOR, RequestMeta};
use crate::scope::{HttpRequest as HttpRequestScope, SCOPE_TOPOLOGY};

/// The axum HTTP protocol definition.
///
/// Accumulates the registered/discovered controllers, contributes no extra DI seeds, and
/// builds the [`AxumRuntime`]: each controller's [`axum::Router`] merged together and
/// wrapped by a per-request scope layer that opens the request scope and threads it into
/// the request extensions for the [`Inject`](crate::Inject) extractor.
#[derive(Default)]
pub struct Axum {
    controllers: Vec<ControllerDescriptor>,

    /// Global middleware, in registration order — both raw `tower::Layer`s (via
    /// [`AxumAppBuilder::layer`]) and DI-resolved [`AxumMiddleware`]s (via
    /// [`AxumAppBuilder::middleware`]) accumulate here, so ordering between the two is just
    /// registration order.
    middleware: Vec<MiddlewareApplier>,

    /// Discovered `#[controller(ws = ..)]` descriptors. Only mounted for protocols a user opts into
    /// via [`register_ws`](AxumAppBuilder::register_ws).
    #[cfg(feature = "ws")]
    ws_controllers: Vec<crate::ws::WsControllerRegistration>,

    /// Opt-in ws endpoints: each pairs a protocol type with the path to mount its upgrade handler.
    #[cfg(feature = "ws")]
    ws_registrations: Vec<WsRegistration>,
}

/// A mount closure: builds one ws endpoint's router + handle from its controllers and runtime. A
/// boxed `FnOnce` (not a `fn` pointer) so it can capture the path and the protocol's `Options`,
/// whose type varies by protocol.
#[cfg(feature = "ws")]
type WsMount = Box<
    dyn FnOnce(
            Vec<crate::ws::WsControllerDescriptor>,
            &AppRuntime,
        ) -> crate::Result<(axum::Router, crate::ws::WebsocketHandler)>
        + Send,
>;

/// One opt-in ws endpoint: a protocol type (by [`TypeId`](std::any::TypeId)) bound to a path, with a
/// [`WsMount`] closure that already captures the path and the protocol's `Options`.
#[cfg(feature = "ws")]
struct WsRegistration {
    path: String,
    protocol: std::any::TypeId,
    protocol_name: &'static str,
    mount: WsMount,
    register: fn(&mut AppRegistry),
}

/// One validated WebSocket endpoint and the exact route plan retained from preparation.
#[cfg(feature = "ws")]
struct PreparedWsRegistration {
    #[cfg(feature = "tooling")]
    path: String,
    #[cfg(feature = "tooling")]
    protocol_name: &'static str,
    controllers: Vec<crate::ws::WsControllerDescriptor>,
    mount: WsMount,
}

/// The validated Axum plan awaiting router and runtime construction.
pub struct PreparedAxum {
    controllers: Vec<ControllerDescriptor>,
    middleware: Vec<MiddlewareApplier>,
    #[cfg(feature = "tooling")]
    base_prefix: String,
    #[cfg(feature = "ws")]
    ws_registrations: Vec<PreparedWsRegistration>,
}

impl ProtocolDefinition for Axum {
    type Prepared = PreparedAxum;
    type Error = crate::Error;

    const ID: upwell_app::ProtocolId =
        upwell_core::namespaced_id!(upwell_app::ProtocolId, "upwell/axum");
    const SCOPE_TOPOLOGY: upwell_app::ScopeTopology = SCOPE_TOPOLOGY;

    fn register(&self, registry: &mut AppRegistry) {
        // Protocol configuration is a builtin: it is present even when the app does not call
        // `auto_discover`, and its field defaults make a missing `[axum]` subtree valid.
        registry
            .config_bindings
            .push(ConfigBinding::of::<AxumConfig>(AXUM_CONFIG_PATH));
        registry.components.push(REQUEST_META_DESCRIPTOR);
        #[cfg(feature = "ws")]
        registry.components.extend([
            crate::ws::WEBSOCKET_UPGRADE_META_DESCRIPTOR,
            crate::ws::WS_CONNECTION_META_DESCRIPTOR,
        ]);

        #[cfg(feature = "openapi")]
        registry
            .config_bindings
            .push(ConfigBinding::of::<crate::OpenApiConfig>(
                crate::AXUM_OPENAPI_CONFIG_PATH,
            ));

        #[cfg(feature = "ws")]
        {
            let mut registered = std::collections::HashSet::new();

            for registration in &self.ws_registrations {
                if registered.insert(registration.protocol) {
                    (registration.register)(registry);
                }
            }
        }
    }

    fn prepare(self, _context: &ValidationContext<'_>) -> crate::Result<Self::Prepared> {
        let config = _context
            .config::<AxumConfig>(AXUM_CONFIG_PATH)
            .expect("AxumConfig missing from config store; Axum should register it")
            .snapshot();
        let base_prefix = normalize_base_path(&config.base_path);

        if !base_prefix.is_empty() {
            crate::config::validate_mount_path("Axum base path", &base_prefix)?;
        }

        #[cfg(feature = "ws")]
        let ws_registrations = {
            crate::ws::validate_config(&config)?;
            let mut prepared = Vec::with_capacity(self.ws_registrations.len());
            let mut endpoints = std::collections::HashSet::new();
            let mut paths = std::collections::HashSet::new();

            for registration in self.ws_registrations {
                crate::config::validate_mount_path("WebSocket mount path", &registration.path)?;

                if !endpoints.insert((registration.protocol, registration.path.clone())) {
                    return Err(crate::Error::Config(format!(
                        "WebSocket protocol `{}` is mounted more than once at `{}`",
                        registration.protocol_name, registration.path
                    )));
                }

                if !paths.insert(registration.path.clone()) {
                    return Err(crate::Error::Config(format!(
                        "more than one WebSocket endpoint is mounted at `{}`",
                        registration.path
                    )));
                }

                let controllers: Vec<crate::ws::WsControllerDescriptor> = self
                    .ws_controllers
                    .iter()
                    .filter(|descriptor| (descriptor.protocol)() == registration.protocol)
                    .map(crate::ws::WsControllerDescriptor::prepare)
                    .collect();

                crate::ws::validate_unique_destinations(&controllers, registration.protocol_name)?;
                prepared.push(PreparedWsRegistration {
                    #[cfg(feature = "tooling")]
                    path: registration.path,
                    #[cfg(feature = "tooling")]
                    protocol_name: registration.protocol_name,
                    controllers,
                    mount: registration.mount,
                });
            }

            prepared
        };

        #[cfg(feature = "openapi")]
        {
            let config = _context
                .config::<crate::OpenApiConfig>(crate::AXUM_OPENAPI_CONFIG_PATH)
                .expect("OpenApiConfig missing from config store; Axum should register it")
                .snapshot();

            crate::openapi::validate_config(&config)?;
        }

        Ok(PreparedAxum {
            controllers: self.controllers,
            middleware: self.middleware,
            #[cfg(feature = "tooling")]
            base_prefix,
            #[cfg(feature = "ws")]
            ws_registrations,
        })
    }

    fn auto_discover(&mut self) {
        self.controllers.extend(CONTROLLERS.iter().copied());

        #[cfg(feature = "ws")]
        self.ws_controllers
            .extend(crate::ws::WS_CONTROLLERS.iter().copied());
    }
}

impl PreparedProtocol for PreparedAxum {
    type Runtime = AxumRuntime;
    type Error = crate::Error;

    fn build(self, runtime: &AppRuntime) -> crate::Result<Self::Runtime> {
        let config = runtime
            .root()
            .config::<AxumConfig>(AXUM_CONFIG_PATH)
            .expect("AxumConfig missing from config store; Axum should register it");
        let config_snapshot = config.snapshot();

        // Merge every controller's routes. Each builder resolves its controller singleton
        // from the runtime and captures it in the route handlers, so the merged router owns
        // ready-to-call handlers.
        let mut router = axum::Router::new();

        for descriptor in &self.controllers {
            router = router.merge((descriptor.router)(runtime));
        }

        // WebSocket endpoints are opt-in: for each `register_ws::<P>(path)`, select the ws
        // controllers that speak `P`, let `P::build` set up its own routing, and merge the
        // path-scoped router. The protocol owns dispatch; we only mount it and keep its handle.
        #[cfg(feature = "ws")]
        let ws_endpoints = {
            let mut endpoints = Vec::with_capacity(self.ws_registrations.len());

            for registration in self.ws_registrations {
                // The mount closure already holds the path and the protocol's options.
                let (ws_router, handler) = (registration.mount)(registration.controllers, runtime)?;

                router = router.merge(ws_router);
                endpoints.push(handler);
            }

            endpoints
        };

        // Global middleware, first-registered outermost (mirroring the RPC protocol's own
        // convention): `axum::Router::layer` stacks last-applied-outermost, so folding in
        // reverse registration order makes the first-registered applier the outermost one.
        // These must stay *inside* (applied before) the scope-open layer below so every piece
        // of user middleware — raw or DI-backed — can `Inject` request-scoped state.
        for applier in self.middleware.into_iter().rev() {
            router = applier(runtime, router);
        }

        // Protocol-wide limits are real router policy, not passive configuration. The body limit
        // feeds Axum's buffered extractors; the timeout covers user middleware and the handler.
        router = router.layer(axum::extract::DefaultBodyLimit::max(
            config_snapshot.max_request_body_bytes,
        ));

        if config_snapshot.request_timeout_ms > 0 {
            let timeout = std::time::Duration::from_millis(config_snapshot.request_timeout_ms);
            router = router.layer(middleware::from_fn(move |request, next: Next| async move {
                tokio::time::timeout(timeout, next.run(request))
                    .await
                    .unwrap_or_else(|_| axum::http::StatusCode::REQUEST_TIMEOUT.into_response())
            }));
        }

        // The bridge: a per-request layer that opens the HttpRequest scope (parented at the
        // singleton root) and inserts its handle into the request extensions. `Inject`
        // reads it back out; a scope-build failure degrades to 500 rather than panicking.
        // Also seeds `RequestMeta` (method/URI/headers/cookies) so request-scoped components
        // and handlers can depend on the native request without axum's own extractors
        // entering the DI graph.
        let scope_runtime = runtime.clone();
        let router = router.layer(middleware::from_fn(
            move |mut request: Request, next: Next| {
                let scope_runtime = scope_runtime.clone();

                async move {
                    let meta = RequestMeta::from_parts(
                        request.method().clone(),
                        request.uri().clone(),
                        request.headers().clone(),
                    );
                    let seed = BoxedComponent {
                        ty: TypeDescriptor::of::<RequestMeta>("RequestMeta"),
                        value: Box::new(meta),
                    };

                    match scope_runtime
                        .open_scope_from_root(&HttpRequestScope, vec![seed])
                        .await
                    {
                        Ok(scope) => {
                            request.extensions_mut().insert(ScopeHandle(scope));

                            next.run(request).await
                        }

                        Err(error) => {
                            tracing::error!(
                                target: "upwell::axum",
                                error = %error,
                                "request scope build failed"
                            );

                            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        }
                    }
                }
            },
        ));

        // One canonical prefix drives both the OpenAPI server/UI URLs and the router nesting, so the
        // documented server, the spec URL a UI fetches, and the actual route all agree.
        let base_prefix = normalize_base_path(&config_snapshot.base_path);

        // The OpenAPI surface (spec + UI) is mounted *outside* the scope layer: it is static and
        // needs no per-request DI scope. Built once from the link-time operation/schema slices.
        #[cfg(feature = "openapi")]
        let router = {
            let openapi_config = runtime
                .root()
                .config::<crate::OpenApiConfig>(crate::AXUM_OPENAPI_CONFIG_PATH)
                .expect("OpenApiConfig missing from config store; Axum should register it")
                .snapshot();

            crate::openapi::mount(router, &openapi_config, &base_prefix)?
        };

        // A configured `base_path` nests the whole application (controllers + OpenAPI) under one
        // prefix. Nesting preserves the inner router's layers, so the scope layer still applies.
        let router = nest_base_path(router, &base_prefix);

        let axum = AxumRuntime::new(router, config);

        #[cfg(feature = "ws")]
        let axum = axum.with_ws_endpoints(ws_endpoints);

        Ok(axum)
    }

    #[cfg(feature = "tooling")]
    fn tooling(&self, contributions: &mut upwell_app::ToolingContributions) {
        use std::collections::BTreeMap;

        use upwell_app::{ResourceDisplay, ToolingEndpoint, ToolingRelationshipKind};

        contributions.display(ResourceDisplay {
            label: Some(String::from("Axum HTTP")),
            group: Some(String::from("Protocols")),
            summary: Some(format!(
                "{} controllers, {} middleware layers",
                self.controllers.len(),
                self.middleware.len()
            )),
            details: BTreeMap::from([(
                String::from("base-path"),
                if self.base_prefix.is_empty() {
                    String::from("/")
                } else {
                    self.base_prefix.clone()
                },
            )]),
        });

        contributions.facet(
            "summary",
            1,
            upwell_app::tooling_schema::JsonValue::Object(
                [
                    (
                        String::from("controller_count"),
                        self.controllers.len().into(),
                    ),
                    (
                        String::from("middleware_count"),
                        self.middleware.len().into(),
                    ),
                    #[cfg(feature = "ws")]
                    (
                        String::from("websocket_endpoint_count"),
                        self.ws_registrations.len().into(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        );

        for controller in &self.controllers {
            let id = format!("controller/{}", controller.id);

            contributions.resource_with_labels(
                &id,
                controller.name,
                BTreeMap::from([
                    (String::from("base-path"), controller.base.to_string()),
                    (String::from("kind"), String::from("http-controller")),
                    (
                        String::from("rust-type"),
                        (controller.ty.type_name)().to_string(),
                    ),
                ]),
            );
            contributions.resource_display(
                &id,
                ResourceDisplay {
                    label: Some(controller.name.to_string()),
                    group: Some(String::from("HTTP controllers")),
                    summary: Some(format!("mounted at {}", controller.base)),
                    details: BTreeMap::from([
                        (String::from("base-path"), controller.base.to_string()),
                        (
                            String::from("rust-type"),
                            (controller.ty.type_name)().to_string(),
                        ),
                    ]),
                },
            );
            contributions.relationship(
                ToolingRelationshipKind::Contains,
                ToolingEndpoint::Owner,
                ToolingEndpoint::Resource(&id),
            );

            for route in (controller.routes)() {
                let route_id = format!(
                    "route/{}/{}/{}/{}",
                    controller.id,
                    route.method.to_ascii_lowercase(),
                    route.path,
                    route.handler
                );
                let path = join_http_paths(&self.base_prefix, controller.base, route.path);

                contributions.resource_with_labels(
                    &route_id,
                    route.handler,
                    BTreeMap::from([(String::from("kind"), String::from("http-route"))]),
                );
                let mut details = BTreeMap::from([
                    (String::from("handler"), route.handler.to_string()),
                    (String::from("method"), route.method.to_string()),
                    (
                        String::from("output"),
                        route
                            .output
                            .ty
                            .map(|output| (output.type_name)().to_string())
                            .unwrap_or_else(|| route.output.declared.to_string()),
                    ),
                    (
                        String::from("output-shape"),
                        format!("{:?}", route.output.shape),
                    ),
                    (String::from("path"), path.clone()),
                ]);
                let responses = route
                    .output
                    .responses
                    .iter()
                    .map(|response| {
                        let mut value = response.status.to_string();

                        if let Some(redirect) = response.redirect {
                            value.push_str(" redirect ");
                            value.push_str(redirect);
                        }

                        match response.body {
                            crate::HttpResponseBodyDescriptor::Empty => value.push_str(" empty"),
                            crate::HttpResponseBodyDescriptor::Typed(body) => {
                                value.push_str(" body ");
                                value.push_str((body.type_name)());
                            }
                            crate::HttpResponseBodyDescriptor::Opaque => value.push_str(" opaque"),
                        }

                        value
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let inputs = route
                    .inputs
                    .iter()
                    .map(|input| {
                        format!(
                            "{} ({:?}): {}",
                            input.name,
                            input.source,
                            (input.ty.type_name)()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let path_parameters = route
                    .path_parameters
                    .iter()
                    .map(|parameter| {
                        if parameter.catch_all {
                            format!("*{}", parameter.name)
                        } else {
                            parameter.name.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                if !inputs.is_empty() {
                    details.insert(String::from("inputs"), inputs);
                }

                if !path_parameters.is_empty() {
                    details.insert(String::from("path-parameters"), path_parameters);
                }

                if !responses.is_empty() {
                    details.insert(String::from("responses"), responses);
                }

                contributions.resource_display(
                    &route_id,
                    ResourceDisplay {
                        label: Some(format!("{} {path}", route.method)),
                        group: Some(format!("HTTP · {}", controller.name)),
                        summary: Some(format!("handler {}", route.handler)),
                        details,
                    },
                );
                contributions.relationship(
                    ToolingRelationshipKind::Contains,
                    ToolingEndpoint::Resource(&id),
                    ToolingEndpoint::Resource(&route_id),
                );
            }
        }

        #[cfg(feature = "ws")]
        for (ordinal, endpoint) in self.ws_registrations.iter().enumerate() {
            let id = format!("websocket/{ordinal}");

            contributions.resource_with_labels(
                &id,
                endpoint.protocol_name,
                BTreeMap::from([
                    (String::from("kind"), String::from("websocket-endpoint")),
                    (String::from("path"), endpoint.path.clone()),
                    (
                        String::from("protocol-name"),
                        endpoint.protocol_name.to_string(),
                    ),
                    (
                        String::from("protocol-type"),
                        endpoint.protocol_name.to_string(),
                    ),
                    (
                        String::from("controller-count"),
                        endpoint.controllers.len().to_string(),
                    ),
                ]),
            );
            contributions.resource_display(
                &id,
                ResourceDisplay {
                    label: Some(format!("WebSocket {}", endpoint.protocol_name)),
                    group: Some(String::from("WebSocket endpoints")),
                    summary: Some(format!("mounted at {}", endpoint.path)),
                    details: BTreeMap::from([
                        (String::from("path"), endpoint.path.clone()),
                        (
                            String::from("controllers"),
                            endpoint.controllers.len().to_string(),
                        ),
                    ]),
                },
            );
            contributions.relationship(
                ToolingRelationshipKind::Contains,
                ToolingEndpoint::Owner,
                ToolingEndpoint::Resource(&id),
            );

            for controller in &endpoint.controllers {
                let controller_id = format!("websocket/{ordinal}/controller/{}", controller.id);

                contributions.resource_with_labels(
                    &controller_id,
                    controller.name,
                    BTreeMap::from([(String::from("kind"), String::from("websocket-controller"))]),
                );
                contributions.resource_display(
                    &controller_id,
                    ResourceDisplay {
                        label: Some(controller.name.to_string()),
                        group: Some(format!("WebSocket · {}", endpoint.protocol_name)),
                        summary: Some(format!("{} message handlers", controller.routes().len())),
                        details: BTreeMap::from([(
                            String::from("rust-type"),
                            (controller.ty.type_name)().to_string(),
                        )]),
                    },
                );
                contributions.relationship(
                    ToolingRelationshipKind::Contains,
                    ToolingEndpoint::Resource(&id),
                    ToolingEndpoint::Resource(&controller_id),
                );

                for route in controller.routes() {
                    let message_id = format!(
                        "websocket/{ordinal}/controller/{}/message/{}",
                        controller.id,
                        route.destination()
                    );
                    let message = route.message();

                    contributions.resource_with_labels(
                        &message_id,
                        route.destination(),
                        BTreeMap::from([(String::from("kind"), String::from("websocket-message"))]),
                    );
                    contributions.resource_display(
                        &message_id,
                        ResourceDisplay {
                            label: Some(format!("message {}", route.destination())),
                            group: Some(format!("WebSocket · {}", controller.name)),
                            summary: Some(
                                message
                                    .map(|message| {
                                        format!(
                                            "{:?} via {}",
                                            message.mode,
                                            (message.codec.type_name)()
                                        )
                                    })
                                    .unwrap_or_else(|| String::from("custom message handler")),
                            ),
                            details: message
                                .map(|message| {
                                    BTreeMap::from([
                                        (String::from("handler"), message.handler.to_string()),
                                        (String::from("mode"), format!("{:?}", message.mode)),
                                        (
                                            String::from("payload"),
                                            message
                                                .payload
                                                .map(|payload| (payload.type_name)().to_string())
                                                .unwrap_or_else(|| String::from("none")),
                                        ),
                                        (
                                            String::from("reply"),
                                            message
                                                .reply
                                                .map(|reply| (reply.type_name)().to_string())
                                                .unwrap_or_else(|| String::from("none")),
                                        ),
                                        (
                                            String::from("codec"),
                                            (message.codec.type_name)().to_string(),
                                        ),
                                    ])
                                })
                                .unwrap_or_default(),
                        },
                    );
                    contributions.relationship(
                        ToolingRelationshipKind::Contains,
                        ToolingEndpoint::Resource(&controller_id),
                        ToolingEndpoint::Resource(&message_id),
                    );
                }
            }
        }
    }
}

#[cfg(feature = "tooling")]
fn join_http_paths(global: &str, controller: &str, route: &str) -> String {
    let segments = [global, controller, route]
        .into_iter()
        .flat_map(|path| path.split('/'))
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        String::from("/")
    } else {
        format!("/{}", segments.join("/"))
    }
}

/// Nests `router` under an already-[normalized](normalize_base_path) global prefix. An empty prefix
/// returns the router unchanged (mounted at the root); otherwise the whole router is nested beneath
/// it, so every route is served under it.
fn nest_base_path(router: axum::Router, prefix: &str) -> axum::Router {
    if prefix.is_empty() {
        router
    } else {
        axum::Router::new().nest(prefix, router)
    }
}

/// Canonicalizes a configured `base_path` into a mount prefix: surrounding slashes trimmed and a
/// single leading `/` ensured, so `api`, `/api`, and `api/` all become `/api`. An empty or `"/"`
/// path yields an empty string (mounted at the root). This guarantees the prefix is a valid
/// [`axum::Router::nest`] path — an unnormalized value like `api` would otherwise panic at
/// construction rather than serving under `/api`.
fn normalize_base_path(base_path: &str) -> String {
    let trimmed = base_path.trim_matches('/');

    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

#[cfg(test)]
mod tests;

/// Configured serving for a built axum app.
///
/// This is the zero-boilerplate counterpart to [`upwell_app::App::serve`]: it binds the
/// listener described by the protocol-owned [`AxumConfig`] instead of requiring a `SocketAddr` at
/// the call site. Explicit `SocketAddr` and pre-bound `TcpListener` serving remain available for
/// tests and advanced embedding.
pub trait AxumAppServe {
    /// Serves on the listener configured at [`AXUM_CONFIG_PATH`].
    fn serve_configured(self) -> impl Future<Output = crate::Result<()>> + Send;
}

impl AxumAppServe for upwell_app::App<Axum> {
    fn serve_configured(self) -> impl Future<Output = crate::Result<()>> + Send {
        self.serve(())
    }
}

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
            .push(Box::new(move |_runtime, router| router.layer(layer)));

        self
    }

    fn middleware<M>(mut self) -> Self
    where
        M: AxumMiddleware + Component<Handle = Arc<M>> + Descriptor<ComponentDescriptor>,
    {
        self.protocol_mut()
            .middleware
            .push(Box::new(|runtime, router| {
                let mw = runtime
                    .root()
                    .get::<M>()
                    .expect("middleware component missing from DI root — did you register it?");

                router.layer(as_layer(mw))
            }));

        self.component::<M>()
    }

    #[cfg(feature = "ws")]
    fn register_ws<P>(self, path: impl Into<String>) -> Self
    where
        P: crate::ws::WebsocketProtocol,
        P::Options: Default,
    {
        self.register_ws_with::<P>(path, P::Options::default())
    }

    #[cfg(feature = "ws")]
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
            protocol: std::any::TypeId::of::<P>(),
            protocol_name: std::any::type_name::<P>(),
            mount,
            register: P::register,
        });

        self
    }
}
