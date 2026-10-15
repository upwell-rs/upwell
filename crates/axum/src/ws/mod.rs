//! WebSocket controllers: the pluggable framing/routing seam and its registration.
//!
//! A `#[controller(ws = P)]` is a DI singleton whose `#[handlers]` `#[message("dest")]` methods
//! contribute **message routes** (a destination → handler map) rather than HTTP routes. The macro
//! emits a [`WsControllerDescriptor`] into the [`WS_CONTROLLERS`] slice and a [`WebsocketController`]
//! impl naming the controller's protocol `P`.
//!
//! WebSockets are **opt-in**: a controller is only mounted when the user activates its protocol on
//! the builder with [`register_ws::<P>(path)`](crate::AxumAppBuilder::register_ws). That call owns
//! the upgrade-endpoint path (it can't be inferred), mounts the framework's generic upgrade handler
//! there, and hands the controllers that speak `P` to [`WebsocketProtocol::build`] — so the protocol
//! sets up its own routing and the app never sees a route. Concrete protocols live in downstream
//! crates such as `upwell-axum-json-ws` and `upwell-axum-stomp`.

pub mod pubsub;

use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, close_code};
use futures::future::BoxFuture;
use tokio::time::Duration;
use upwell_app::{AppRegistry, AppRuntime};
use upwell_config::ContainerConfigExt;
use upwell_core::TypeDescriptor;
use upwell_di::{BoxedComponent, ScopeContainer};

/// How long the framework waits for a WS close handshake to flush before abandoning the socket.
/// Bounds [`mount_ws`]'s error-path close send so a peer that never drains its receive buffer
/// can't block the upgrade task forever.
pub const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// The boxed future a [`WsHandlerFn`] returns — a decoded, dispatched message [`Outcome`], generic
/// over the protocol `P` that owns the payload/outcome vocabulary.
///
/// [`Outcome`]: WebsocketProtocol::Outcome
pub type WsFuture<P> =
    BoxFuture<'static, Result<<P as WebsocketProtocol>::Outcome, WsDispatchError>>;

/// A type-erased message handler for protocol `P`. It is handed the decoded
/// [`Payload`](WebsocketProtocol::Payload) and the message's
/// [`WebsocketMessage`-scope](crate::scope::WebsocketMessage) container, so it can decode the payload into the
/// handler's parameter *and* resolve the handler's `Inject<T>` parameters from the scope chain
/// (message → connection → singleton) before running the
/// controller method (the singleton captured by `Arc`) and turning the response into `P`'s
/// [`Outcome`](WebsocketProtocol::Outcome).
pub type WsHandlerFn<P> = Arc<
    dyn Fn(<P as WebsocketProtocol>::Payload, Arc<ScopeContainer>) -> WsFuture<P> + Send + Sync,
>;

/// What can go wrong dispatching one message, independent of the wire protocol framing it.
#[derive(Debug, thiserror::Error)]
pub enum WsDispatchError {
    /// The destination named by an inbound frame matches no `#[message]` handler.
    #[error("no handler for ws destination `{0}`")]
    NotFound(String),

    /// The payload could not be decoded into the handler's parameter type.
    #[error("decoding ws payload: {0}")]
    Decode(String),

    /// An `Inject<T>` parameter could not be resolved from the message scope.
    #[error("injecting ws dependency: {0}")]
    Inject(String),

    /// The handler's response could not be encoded.
    #[error("encoding ws response: {0}")]
    Encode(String),

    /// The handler returned an application-level error.
    ///
    /// Application error text is deliberately discarded at the generated handler boundary. Its
    /// [`Display`](std::fmt::Display) output may contain credentials or other request-specific
    /// secrets, and retaining it here would make those details available to protocol logging.
    #[error("ws application error")]
    Application,
}

impl WsDispatchError {
    /// A stable, client-safe summary for a directed error reply. Framework diagnostics for
    /// `Inject`, `Decode`, and `Encode` remain available to server logs; application error details
    /// are discarded before reaching the protocol layer.
    pub fn public_message(&self) -> &str {
        match self {
            Self::NotFound(_) => "no handler for destination",
            Self::Decode(_) => "invalid request payload",
            Self::Inject(_) | Self::Encode(_) => "internal error",
            Self::Application => "request failed",
        }
    }
}

/// One message route for protocol `P`: a destination string mapped to its handler. A
/// `#[message("dest")]` method produces one of these (with the controller singleton already
/// captured).
pub struct WsRoute<P: WebsocketProtocol> {
    /// The destination this handler answers (e.g. `"chat.send"`).
    pub destination: &'static str,

    /// The handler, ready to call with a decoded [`Payload`](WebsocketProtocol::Payload).
    pub handler: WsHandlerFn<P>,
}

impl<P: WebsocketProtocol> WsRoute<P> {
    /// Builds a route from a destination and its handler. Called by generated `#[message]` code.
    pub fn new(destination: &'static str, handler: WsHandlerFn<P>) -> Self {
        Self {
            destination,
            handler,
        }
    }
}

/// One `#[handlers]` block's message-route builder, tagged with its controller type `C` and
/// protocol `P` — the WebSocket analog of [`ControllerRoute`](crate::ControllerRoute). Wraps the
/// bare builder fn pointer so it can be an [`UpwellDescriptor`] and thus a
/// `DescriptorFor<C, ControllerWsRoute<C, P>>` bucket element on the `inventory` backend. `Copy` is
/// manual (a naive derive would wrongly demand `C: Copy` / `P: Copy`).
pub struct ControllerWsRoute<C, P: WebsocketProtocol>(
    pub fn() -> Vec<WsRouteDescriptor>,
    std::marker::PhantomData<fn() -> (C, P)>,
);

impl<C, P: WebsocketProtocol> ControllerWsRoute<C, P> {
    /// Wraps one generated route-descriptor group for controller `C` and protocol `P`.
    pub const fn new(routes: fn() -> Vec<WsRouteDescriptor>) -> Self {
        Self(routes, std::marker::PhantomData)
    }
}

impl<C, P: WebsocketProtocol> Clone for ControllerWsRoute<C, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C, P: WebsocketProtocol> Copy for ControllerWsRoute<C, P> {}

impl<C: 'static, P: WebsocketProtocol> upwell_core::UpwellDescriptor for ControllerWsRoute<C, P> {}

type ErasedWsHandler = Box<dyn Any + Send + Sync>;

/// One authoritative WebSocket route declaration before controller construction.
///
/// The destination is available during protocol preparation. The private factory resolves the
/// controller and creates the corresponding typed handler only after the application runtime exists.
#[derive(Clone)]
pub struct WsRouteDescriptor {
    destination: &'static str,
    message: Option<WsMessageDescriptor>,
    handler: Arc<dyn Fn(&AppRuntime) -> ErasedWsHandler + Send + Sync>,
}

/// Resolved behavior of one WebSocket message handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WsMessageMode {
    /// Fire-and-forget message send.
    Send,
    /// Request expecting one encoded reply.
    Request,
}

/// Static semantic metadata for one WebSocket message handler.
#[derive(Clone, Copy, Debug)]
pub struct WsMessageDescriptor {
    /// Rust handler method name.
    pub handler: &'static str,
    /// Protocol destination.
    pub destination: &'static str,
    /// Decoded message payload type, absent for payload-less handlers.
    pub payload: Option<TypeDescriptor>,
    /// Resolved send/request behavior.
    pub mode: WsMessageMode,
    /// Encoded reply value type for request handlers.
    pub reply: Option<TypeDescriptor>,
    /// Codec used symmetrically for payload and reply bodies.
    pub codec: TypeDescriptor,
}

impl WsRouteDescriptor {
    /// Creates a route declaration whose destination and handler factory cannot diverge.
    pub fn new<P: WebsocketProtocol>(
        destination: &'static str,
        handler: fn(&AppRuntime) -> WsHandlerFn<P>,
    ) -> Self {
        Self {
            destination,
            message: None,
            handler: Arc::new(move |runtime| Box::new(handler(runtime))),
        }
    }

    /// Creates a route declaration with retained semantic message metadata.
    pub fn new_described<P: WebsocketProtocol>(
        message: WsMessageDescriptor,
        handler: fn(&AppRuntime) -> WsHandlerFn<P>,
    ) -> Self {
        Self {
            destination: message.destination,
            message: Some(message),
            handler: Arc::new(move |runtime| Box::new(handler(runtime))),
        }
    }

    /// The destination claimed by this route.
    pub fn destination(&self) -> &'static str {
        self.destination
    }

    /// Returns semantic message metadata when supplied by the route author or macro.
    pub fn message(&self) -> Option<&WsMessageDescriptor> {
        self.message.as_ref()
    }

    fn to_route<P: WebsocketProtocol>(&self, runtime: &AppRuntime) -> WsRoute<P> {
        let handler = (self.handler)(runtime)
            .downcast::<WsHandlerFn<P>>()
            .unwrap_or_else(|_| {
                panic!(
                    "ws route `{}` built a handler for the wrong protocol `{}`",
                    self.destination,
                    std::any::type_name::<P>()
                )
            });

        WsRoute::new(self.destination, *handler)
    }
}

/// Rejects ambiguous destinations before calling [`WebsocketProtocol::build`]. Keeping this check
/// in the framework preserves the original infallible public `build` contract for downstream
/// protocols while preventing bundled or custom implementations from silently depending on link
/// order when routes collide.
/// Turns a handler's response value `R` into this protocol's [`Outcome`](WebsocketProtocol::Outcome).
/// The macro calls `<P as WsRespond<R>>::respond(response)` for send-mode handlers.
pub trait WsRespond<R>: WebsocketProtocol {
    /// Renders `response` into this protocol's send outcome.
    fn respond(response: R) -> Result<Self::Outcome, WsDispatchError>;
}

/// A ws controller's link-time registration: its identity, the protocol it speaks, and a builder
/// for its message routes. Mirrors [`ControllerDescriptor`](crate::ControllerDescriptor), but the
/// upgrade *path* is **not** here — it comes from `register_ws`.
#[derive(Clone, Copy)]
pub struct WsControllerRegistration {
    /// The controller's id (defaults to the lowercased type name).
    pub id: &'static str,

    /// The controller's display name (the type name).
    pub name: &'static str,

    /// The controller's concrete type.
    pub ty: TypeDescriptor,

    /// The [`TypeId`] of the [`WebsocketProtocol`] this controller speaks, so `register_ws::<P>`
    /// selects exactly the controllers framed by `P`. A `fn` (not a const) because `TypeId::of`
    /// is not yet const.
    pub protocol: fn() -> TypeId,

    /// The protocol's type name, for diagnostics. A `fn` because `type_name` is not yet const.
    pub protocol_name: fn() -> &'static str,

    /// Returns authoritative route declarations without constructing the controller.
    pub routes: fn() -> Vec<WsRouteDescriptor>,
}

/// A prepared WebSocket controller descriptor passed to [`WebsocketProtocol::build`].
#[derive(Clone)]
pub struct WsControllerDescriptor {
    /// The controller's stable id.
    pub id: &'static str,
    /// The controller's display name.
    pub name: &'static str,
    /// The controller's concrete type.
    pub ty: TypeDescriptor,
    /// The controller's WebSocket protocol type.
    pub protocol: TypeId,
    /// The protocol's type name for diagnostics.
    pub protocol_name: &'static str,
    routes: Arc<[WsRouteDescriptor]>,
}

impl WsControllerDescriptor {
    pub(crate) fn prepare(registration: &WsControllerRegistration) -> Self {
        Self {
            id: registration.id,
            name: registration.name,
            ty: registration.ty,
            protocol: (registration.protocol)(),
            protocol_name: (registration.protocol_name)(),
            routes: (registration.routes)().into(),
        }
    }

    /// The authoritative route declarations retained during application preparation.
    pub fn routes(&self) -> &[WsRouteDescriptor] {
        &self.routes
    }

    /// Builds this controller's typed routes from the declarations retained during preparation.
    pub fn routes_for<P: WebsocketProtocol>(&self, runtime: &AppRuntime) -> Vec<WsRoute<P>> {
        assert_eq!(
            self.protocol,
            TypeId::of::<P>(),
            "ws controller `{}` routes requested for the wrong protocol `{}`",
            self.name,
            std::any::type_name::<P>()
        );

        self.routes
            .iter()
            .map(|route| route.to_route(runtime))
            .collect()
    }
}

/// Rejects ambiguous destinations before component or protocol runtime construction.
pub(crate) fn validate_unique_destinations(
    controllers: &[WsControllerDescriptor],
    protocol_name: &str,
) -> crate::Result<()> {
    let mut destinations = HashSet::new();

    for controller in controllers {
        for route in controller.routes() {
            let destination = route.destination();

            if !destinations.insert(destination) {
                return Err(crate::Error::Config(format!(
                    "duplicate WebSocket destination `{destination}` for protocol `{protocol_name}`"
                )));
            }
        }
    }

    Ok(())
}

/// The link-time slice every `#[controller(ws = ..)]` registers into, mirroring [`CONTROLLERS`].
///
/// [`CONTROLLERS`]: crate::CONTROLLERS
#[linkme::distributed_slice]
pub static WS_CONTROLLERS: [WsControllerRegistration];

/// Implemented by every `#[controller(ws = P)]` struct: it names its protocol and builds its
/// message routes. Generated alongside the [`WsControllerDescriptor`]; the per-`#[handlers]`-block
/// assertion forces this to hold so a REST controller can never be given message routing.
pub trait WebsocketController {
    /// The protocol that frames and routes this controller's messages.
    type Protocol: WebsocketProtocol;
}

/// A pluggable WebSocket sub-protocol: it owns framing (how a raw [`Message`](axum::extract::ws::Message)
/// maps to a destination + payload) and routing (how a destination maps to a handler). Driven
/// generically — never stored as a trait object — so it carries associated state freely.
///
/// [`build`](Self::build) receives every controller registered to this protocol type and sets up
/// its routing once; [`serve`](Self::serve) then drives one upgraded socket against it. The future
/// A concrete implementation may add request correlation, subscriptions, or another framing model
/// without changing this seam.
pub trait WebsocketProtocol: Send + Sync + Sized + 'static {
    /// The decoded body a handler receives or an outbound frame carries.
    type Payload: Send + 'static;

    /// What a handler returns before protocol framing.
    type Outcome: Send + 'static;

    /// Per-endpoint settings passed at registration. A user supplies these through
    /// [`register_ws_with`](crate::AxumAppBuilder::register_ws_with); the plain
    /// [`register_ws`](crate::AxumAppBuilder::register_ws) uses [`Default`].
    type Options: Send + 'static;

    /// A typed failure raised while constructing the protocol's endpoint state.
    type BuildError: std::error::Error + Send + Sync + 'static;

    /// RFC 6455 subprotocol tokens accepted by this protocol, in server preference order.
    const SUBPROTOCOLS: &'static [&'static str] = &[];

    /// Whether an upgrade must negotiate one of [`SUBPROTOCOLS`](Self::SUBPROTOCOLS).
    const REQUIRE_SUBPROTOCOL: bool = false;

    /// Builds the protocol's routing from prepared controllers and endpoint `options`. Called once
    /// per `register_ws` entrypoint at app build. The protocol keeps whatever
    /// it needs from `runtime` (e.g. a clone, to open per-message
    /// [`WebsocketMessage`](crate::scope::WebsocketMessage) scopes while serving).
    fn build(
        controllers: &[WsControllerDescriptor],
        runtime: &AppRuntime,
        options: Self::Options,
    ) -> Result<Self, Self::BuildError>;

    /// Drives one upgraded connection until the peer closes it or graceful shutdown fires.
    /// `connection` is this socket's
    /// [`WebsocketConnection`](crate::scope::WebsocketConnection) scope (opened once by the
    /// framework); the protocol parents each per-message scope at it.
    fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: Arc<ScopeContainer>,
        shutdown: WsShutdown,
    ) -> impl Future<Output = ()> + Send;

    /// Contributes protocol-owned DI components before the root container is validated and built.
    fn register(_registry: &mut AppRegistry) {}
}

/// Framework-owned controls resolved from each WebSocket connection's config store. This remains
/// internal so adding operational controls does not change the public protocol implementation
/// contract.
#[derive(Clone, Copy, Debug)]
pub struct WsConnectionSettings {
    idle_timeout: Option<Duration>,
}

impl WsConnectionSettings {
    /// Reads framework-owned WebSocket settings from a connection scope.
    pub fn from_connection(connection: &ScopeContainer) -> Self {
        let timeout_ms = connection
            .config::<crate::AxumConfig>(crate::AXUM_CONFIG_PATH)
            .map(|config| config.snapshot().websocket_idle_timeout_ms)
            .unwrap_or_else(|| crate::AxumConfig::default().websocket_idle_timeout_ms);

        Self {
            idle_timeout: (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms)),
        }
    }

    /// The configured idle interval, or `None` when liveness probing is disabled.
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }
}

/// Native HTTP metadata captured from the request that initiated a WebSocket upgrade.
#[derive(Clone, Debug)]
pub struct WebsocketUpgradeMeta {
    /// The upgrade request's HTTP method.
    pub method: axum::http::Method,

    /// The upgrade request's URI.
    pub uri: axum::http::Uri,

    /// The upgrade request's headers.
    pub headers: axum::http::HeaderMap,

    /// Cookies parsed from the upgrade request's `Cookie` headers.
    pub cookies: std::collections::HashMap<String, String>,
}

impl WebsocketUpgradeMeta {
    /// Captures metadata from a WebSocket upgrade request.
    pub fn from_parts(
        method: axum::http::Method,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
    ) -> Self {
        let request = crate::RequestMeta::from_parts(method, uri, headers);

        Self {
            method: request.method,
            uri: request.uri,
            headers: request.headers,
            cookies: request.cookies,
        }
    }
}

impl upwell_di::Injectable for WebsocketUpgradeMeta {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[cfg(feature = "di-check")]
impl upwell_di::Provide<WebsocketUpgradeMeta> for upwell_di::Wiring {}

pub(crate) static WEBSOCKET_UPGRADE_META_DESCRIPTOR: upwell_di::ComponentDescriptor =
    upwell_di::ComponentDescriptor::manual(
        "__upwell_websocket_upgrade_meta",
        "WebsocketUpgradeMeta",
        TypeDescriptor::of::<WebsocketUpgradeMeta>("WebsocketUpgradeMeta"),
        &crate::scope::WebsocketConnection,
    );

/// Metadata selected while accepting one WebSocket upgrade.
#[derive(Clone, Debug)]
pub struct WsConnectionMeta {
    selected_subprotocol: Option<String>,
}

impl WsConnectionMeta {
    /// The negotiated RFC 6455 subprotocol, if one was selected.
    pub fn selected_subprotocol(&self) -> Option<&str> {
        self.selected_subprotocol.as_deref()
    }
}

impl upwell_di::Injectable for WsConnectionMeta {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[cfg(feature = "di-check")]
impl upwell_di::Provide<WsConnectionMeta> for upwell_di::Wiring {}

pub(crate) static WS_CONNECTION_META_DESCRIPTOR: upwell_di::ComponentDescriptor =
    upwell_di::ComponentDescriptor::manual(
        "__upwell_ws_connection_meta",
        "WsConnectionMeta",
        TypeDescriptor::of::<WsConnectionMeta>("WsConnectionMeta"),
        &crate::scope::WebsocketConnection,
    );

/// Tracks peer activity without spawning a feeder or timer task. A silent connection is probed
/// once, then dropped if no frame (normally a pong) arrives during the following interval.
pub struct WsIdle {
    timeout: Option<Duration>,
    deadline: Option<tokio::time::Instant>,
    awaiting_probe_reply: bool,
}

impl WsIdle {
    /// Builds the liveness state from one connection's framework settings.
    pub fn from_connection(connection: &ScopeContainer) -> Self {
        Self::new(WsConnectionSettings::from_connection(connection).idle_timeout)
    }

    /// Builds liveness state for an explicit idle interval.
    pub fn new(timeout: Option<Duration>) -> Self {
        let deadline = timeout.map(|timeout| tokio::time::Instant::now() + timeout);

        Self {
            timeout,
            deadline,
            awaiting_probe_reply: false,
        }
    }

    /// Waits until the current idle deadline, or forever when probing is disabled.
    pub async fn wait(&self) {
        match self.deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    }

    /// Records peer activity and resets the liveness deadline.
    pub fn on_activity(&mut self) {
        self.awaiting_probe_reply = false;
        self.reset_deadline();
    }

    /// Returns `true` when the peer already ignored one probe and must be disconnected.
    pub fn on_timeout(&mut self) -> bool {
        if self.awaiting_probe_reply {
            return true;
        }

        self.awaiting_probe_reply = true;
        self.reset_deadline();

        false
    }

    fn reset_deadline(&mut self) {
        self.deadline = self
            .timeout
            .map(|timeout| tokio::time::Instant::now() + timeout);
    }
}

/// Per-endpoint WebSocket admission gate. An owned permit is captured by the upgrade future and is
/// released automatically when that connection finishes, including scope-build and panic unwind.
#[derive(Clone)]
struct WsAdmission {
    permits: Option<Arc<tokio::sync::Semaphore>>,
}

impl WsAdmission {
    fn new(max_connections: usize) -> crate::Result<Self> {
        if max_connections > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(crate::Error::Config(format!(
                "max_websocket_connections ({max_connections}) exceeds Tokio's semaphore limit ({})",
                tokio::sync::Semaphore::MAX_PERMITS
            )));
        }

        Ok(Self {
            permits: (max_connections > 0)
                .then(|| Arc::new(tokio::sync::Semaphore::new(max_connections))),
        })
    }

    fn try_acquire(
        &self,
    ) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, tokio::sync::TryAcquireError> {
        self.permits
            .as_ref()
            .map(|permits| Arc::clone(permits).try_acquire_owned())
            .transpose()
    }
}

/// Temporary protocol-level guard until generic extraction-time validation is available.
pub(crate) fn validate_config(config: &crate::AxumConfig) -> crate::Result<()> {
    if config.max_websocket_message_bytes == 0 || config.max_websocket_frame_bytes == 0 {
        return Err(crate::Error::Config(
            "WebSocket message and frame byte limits must both be greater than zero".to_owned(),
        ));
    }

    if config.max_websocket_connections > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(crate::Error::Config(format!(
            "max_websocket_connections ({}) exceeds Tokio's semaphore limit ({})",
            config.max_websocket_connections,
            tokio::sync::Semaphore::MAX_PERMITS
        )));
    }

    Ok(())
}

/// A [`WebsocketProtocol`] that carries topic pub/sub: it frames a delivered message for one
/// subscriber. This is the server-side companion to [`MessagingProtocol`](crate::messaging::MessagingProtocol)
/// (which supplies the wire body and default codec); together they let the neutral
/// [`SubscriptionRegistry`](crate::ws::pubsub::SubscriptionRegistry) and
/// [`TopicBus`](crate::ws::pubsub::TopicBus) fan out for any protocol. STOMP is one implementation;
/// a new protocol adds its own `frame_message` without touching the registry/bus. Behind `ws` (not
/// `stomp`), so a non-STOMP protocol implements it without enabling STOMP.
pub trait PubSubProtocol: WebsocketProtocol + crate::messaging::MessagingProtocol {
    /// The outbound frame this protocol delivers to a subscriber's writer task.
    type OutFrame: Send + 'static;

    /// Frames one delivery: the registry supplies a fresh `message_id` and the target's `sub_id`;
    /// the protocol renders its wire frame from the encoded `body` and any extra `headers`.
    fn frame_message(
        message_id: u64,
        destination: &str,
        sub_id: &str,
        body: &Self::Body,
        headers: &[(String, String)],
    ) -> Self::OutFrame;
}

/// A [`PubSubProtocol`] that can direct a point-to-point reply back to a requester — the server-side
/// companion to the client's [`MessageRequest`](crate::client::MessageRequest). A `#[message]`
/// handler that returns a non-unit value has its (codec-encoded) return wrapped by
/// [`reply`](Self::reply) into an outcome the protocol routes to the *requester* (STOMP: via the
/// inbound frame's `reply-to`/`correlation-id`), never broadcast. A handler returning `()` uses
/// [`WsRespond`] instead, so a protocol without request/response simply never needs this.
pub trait MessageReply: WebsocketProtocol + crate::messaging::MessagingProtocol {
    /// Wraps an already-encoded reply `body` into this protocol's outcome.
    fn reply(body: Self::Body) -> Self::Outcome;
}

/// A connection-side graceful-shutdown signal. A protocol's [`serve`](WebsocketProtocol::serve) loop
/// races [`wait`](Self::wait) against reading the socket, so it can drain on app shutdown rather than
/// blocking the server's graceful stop on a long-lived connection.
#[derive(Clone)]
pub struct WsShutdown(tokio::sync::watch::Receiver<bool>);

impl WsShutdown {
    /// Resolves when graceful shutdown has been signalled for this endpoint.
    pub async fn wait(&mut self) {
        // Already-signalled, or wait for the next change; either way, return so the caller drains.
        if *self.0.borrow() {
            return;
        }

        let _ = self.0.changed().await;
    }
}

/// The app-side management handle for one mounted ws endpoint: its path and protocol, and the
/// trigger that drains its live connections on graceful shutdown. Held by the [`Axum`](crate::Axum)
/// protocol so endpoints can be inspected and shut down.
pub struct WebsocketHandler {
    path: String,
    protocol_name: &'static str,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl WebsocketHandler {
    /// The upgrade path this endpoint is mounted at.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The name of the protocol serving this endpoint.
    pub fn protocol_name(&self) -> &'static str {
        self.protocol_name
    }

    /// Signals every live connection on this endpoint to drain and close.
    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

/// Mounts one ws endpoint: builds the protocol `P` from its controllers and `options`, wires the
/// framework's generic upgrade handler at `path`, and returns a path-scoped router plus its
/// management handle. Monomorphized per `P`; `register_ws` stores a closure that calls it.
pub(crate) fn mount_ws<P: WebsocketProtocol>(
    path: &str,
    controllers: Vec<WsControllerDescriptor>,
    runtime: &AppRuntime,
    options: P::Options,
) -> crate::Result<(axum::Router, WebsocketHandler)> {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::response::IntoResponse as _;

    let (tx, rx) = tokio::sync::watch::channel(false);
    let config = runtime
        .root()
        .config::<crate::AxumConfig>(crate::AXUM_CONFIG_PATH)
        .expect("AxumConfig missing from config store; Axum should register it")
        .snapshot();

    validate_config(&config)?;

    let proto = Arc::new(P::build(&controllers, runtime, options).map_err(|source| {
        crate::Error::WebsocketBuild {
            protocol: std::any::type_name::<P>(),
            source: Box::new(source),
        }
    })?);
    let shutdown = WsShutdown(rx);
    let admission = WsAdmission::new(config.max_websocket_connections)?;
    let max_message_bytes = config.max_websocket_message_bytes;
    let max_frame_bytes = config.max_websocket_frame_bytes;
    let runtime = runtime.clone();

    // The pre-built generic upgrade handler opens this socket's WebsocketConnection scope with
    // upgrade and negotiation metadata, then hands the socket to the protocol that owns the
    // read→decode→dispatch→encode→send loop.
    let route_handler = move |method: axum::http::Method,
                              uri: axum::http::Uri,
                              headers: axum::http::HeaderMap,
                              ws: WebSocketUpgrade| {
        let proto = Arc::clone(&proto);
        let shutdown = shutdown.clone();
        let runtime = runtime.clone();
        let admission = admission.clone();

        async move {
            let permit = match admission.try_acquire() {
                Ok(permit) => permit,

                Err(_) => {
                    tracing::warn!(
                        target: "upwell::axum",
                        "websocket connection limit reached; rejecting upgrade"
                    );

                    return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
            };

            let ws = ws
                .max_message_size(max_message_bytes)
                .max_frame_size(max_frame_bytes)
                .protocols(P::SUBPROTOCOLS.iter().copied());
            let selected_subprotocol = ws
                .selected_protocol()
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);

            if P::REQUIRE_SUBPROTOCOL && selected_subprotocol.is_none() {
                return axum::http::StatusCode::BAD_REQUEST.into_response();
            }

            ws.on_upgrade(move |mut socket| async move {
                // Keep the admission permit for exactly the lifetime of this upgraded connection.
                let _permit = permit;
                let upgrade_seed = BoxedComponent {
                    ty: TypeDescriptor::of::<WebsocketUpgradeMeta>("WebsocketUpgradeMeta"),
                    value: Box::new(WebsocketUpgradeMeta::from_parts(method, uri, headers)),
                };
                let connection_seed = BoxedComponent {
                    ty: TypeDescriptor::of::<WsConnectionMeta>("WsConnectionMeta"),
                    value: Box::new(WsConnectionMeta {
                        selected_subprotocol,
                    }),
                };

                let connection = match runtime
                    .open_scope_from_root(
                        &crate::scope::WebsocketConnection,
                        vec![upgrade_seed, connection_seed],
                    )
                    .await
                {
                    Ok(scope) => scope,

                    Err(error) => {
                        tracing::error!(
                            target: "upwell::axum",
                            %error,
                            "ws connection scope build failed; closing socket"
                        );

                        let close = Message::Close(Some(CloseFrame {
                            code: close_code::ERROR,
                            reason: Utf8Bytes::from_static("connection scope build failed"),
                        }));

                        let _ = tokio::time::timeout(SOCKET_SEND_TIMEOUT, socket.send(close)).await;

                        return;
                    }
                };

                proto.serve(socket, connection, shutdown).await;
            })
            .into_response()
        }
    };

    let router = axum::Router::new().route(path, axum::routing::any(route_handler));
    let handler = WebsocketHandler {
        path: path.to_string(),
        protocol_name: std::any::type_name::<P>(),
        shutdown: tx,
    };

    Ok((router, handler))
}

#[cfg(test)]
mod tests;
