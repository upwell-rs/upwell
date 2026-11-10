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

mod connection;
mod meta;
mod mount;
mod prepared;
pub mod pubsub;
mod settings;

use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use axum::extract::ws::WebSocket;
use futures::future::BoxFuture;
use tokio::time::Duration;
use upwell_app::{AppRegistry, AppRuntime, EntryScope};
use upwell_core::{DependencyDescriptor, EntryKey, ItemSource, TypeDescriptor};

use crate::scope::WebsocketMessage;

pub use connection::WsConnectionScope;
pub use meta::{WebsocketUpgradeMeta, WsConnectionMeta};
pub(crate) use meta::{register_framework_seeds, upgrade_seeds};
pub use mount::WebsocketHandler;
pub(crate) use mount::mount_ws;
pub use prepared::{PreparedConnection, scope_failure_close};
pub(crate) use settings::validate_config;
pub use settings::{WsConnectionSettings, WsIdle};

/// How long a protocol waits for a WS frame, such as a close handshake, to flush before
/// abandoning the socket, so a peer that never drains its receive buffer can't block the
/// connection's task forever.
pub const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// The boxed future a [`WsHandlerFn`] returns — a decoded, dispatched message [`Outcome`], generic
/// over the protocol `P` that owns the payload/outcome vocabulary.
///
/// [`Outcome`]: WebsocketProtocol::Outcome
pub type WsFuture<P> =
    BoxFuture<'static, Result<<P as WebsocketProtocol>::Outcome, WsDispatchError>>;

/// A type-erased message handler for protocol `P`. It is handed the decoded
/// [`Payload`](WebsocketProtocol::Payload) and the message's
/// [`WebsocketMessage`-scope](crate::scope::WebsocketMessage) instance, so it can decode the payload into the
/// handler's parameter *and* resolve the handler's `Inject<T>` parameters from the scope chain
/// (message → connection → singleton) before running the
/// controller method (the singleton captured by `Arc`) and turning the response into `P`'s
/// [`Outcome`](WebsocketProtocol::Outcome).
pub type WsHandlerFn<P> = Arc<
    dyn Fn(<P as WebsocketProtocol>::Payload, EntryScope<WebsocketMessage>) -> WsFuture<P>
        + Send
        + Sync,
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
    /// The handler method's declaration, when the registering macro recorded it.
    pub source: Option<ItemSource>,
    /// The handler's stable entry identity: `ws/<controller ID>.<handler>`.
    pub key: EntryKey,
    /// The dependency edges of the handler's `Inject<T>` parameters, in parameter order.
    pub targets: fn() -> Vec<DependencyDescriptor>,
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
    /// it needs from `runtime`; per-message scopes open through the [`WsConnectionScope`]
    /// each connection opens.
    fn build(
        controllers: &[WsControllerDescriptor],
        runtime: &AppRuntime,
        options: Self::Options,
    ) -> Result<Self, Self::BuildError>;

    /// Drives one upgraded connection until the peer closes it or graceful shutdown fires.
    ///
    /// `connection` is this socket's
    /// [`WebsocketConnection`](crate::scope::WebsocketConnection) scope before it opens. The
    /// protocol decides when to [`open`](PreparedConnection::open) it: at once when it has no
    /// handshake, or after an in-band handshake with what the handshake produced, so no
    /// connection-scoped component observes the connection before it is accepted. It opens
    /// each per-message scope under the opened scope with [`WsConnectionScope::message`]. The
    /// opening must run on the task `serve` runs on, so the instance joins the connection's
    /// invocation context.
    fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: PreparedConnection,
        shutdown: WsShutdown,
    ) -> impl Future<Output = ()> + Send;

    /// Contributes protocol-owned DI components, such as the seed slots the protocol fills,
    /// before the root container is validated and built. The seeds an endpoint fills are
    /// declared by [`seed_claims`](Self::seed_claims), not here: every endpoint opens the
    /// connection and message scopes, so a plain [`AppRegistry::claim_seed`] for them does not
    /// count, and only what each endpoint declares covers its own instances.
    fn register(_registry: &mut AppRegistry) {}

    /// Rejects endpoint `options` that can never work, such as a claimed seed nothing would
    /// produce, while the application prepares, before any endpoint is built, so tooling
    /// reports it too. [`build`](Self::build) may check the same again. Accepts every option by
    /// default.
    ///
    /// # Errors
    ///
    /// The protocol's build error for the options it rejects.
    fn check_options(_options: &Self::Options) -> Result<(), Self::BuildError> {
        Ok(())
    }

    /// Every seed one endpoint configured with `options` fills in each
    /// [`WebsocketConnection`](crate::scope::WebsocketConnection) and
    /// [`WebsocketMessage`](crate::scope::WebsocketMessage) scope it opens, beyond the upgrade
    /// metadata: the protocol's own seeds and the application values its handshake seeds. The
    /// endpoint is declared an opener of both scopes with these seeds, and a value one endpoint
    /// seeds does not count for another's instances. None by default.
    fn seed_claims(_options: &Self::Options) -> Vec<upwell_app::SeedClaim> {
        Vec::new()
    }
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
pub struct WsShutdown(pub(crate) tokio::sync::watch::Receiver<bool>);

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

#[cfg(test)]
mod tests;
