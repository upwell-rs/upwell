//! STOMP 1.2 pub/sub over WebSocket.
//!
//! [`Stomp`] is a [`WebsocketProtocol`](crate::ws::WebsocketProtocol) implementation that adds a
//! broker on top of the shared ws seam: a `SEND` to an app destination (`/app/**`) invokes a
//! `#[message]` handler, a `SUBSCRIBE` registers interest, and a `SEND` to a broker destination
//! (`/topic/**`, `/queue/**`) — or an app handler publishing through a [`Publisher`] — fans a
//! `MESSAGE` out to every subscriber, across connections.
//!
//! Framing is delegated to the [`stomp-parser`](https://crates.io/crates/stomp-parser) crate;
//! this module owns the broker, the connection serve loop, DI scope seeding, and the typed
//! [`Topic`]/[`Publisher`] publish surface. Message-body serialization is pluggable per topic set
//! via [`StompCodec`] (`#[topics(codec = ..)]`), defaulting to [`JsonCodec`].
//!
//! v1 covers the core pub/sub path; see `docs/stomp.md` for what is deferred (RECEIPT,
//! heart-beating, ACK modes, transactions, destination wildcards).

#[path = "server/auth.rs"]
mod auth;
#[path = "server/body.rs"]
mod body;
#[path = "server/broker.rs"]
mod broker;
#[path = "server/error.rs"]
mod error;
#[path = "server/headers.rs"]
mod headers;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use stomp_parser::client::ClientFrame;
use stomp_parser::headers::{HeaderValue, StompVersion, StompVersions};
use stomp_parser::server::{ConnectedFrameBuilder, ErrorFrame, ReceiptFrameBuilder};
use tokio::sync::mpsc;
use upwell_axum::axum::extract::ws::{Message, WebSocket};
use upwell_axum::{AppRuntime, ComponentDescriptor, Scope, TypeDescriptor, WebsocketMessageScope};

use upwell_axum::{
    MessageReply, PubSubProtocol, SOCKET_SEND_TIMEOUT, WebsocketProtocol, WsConnectionScope,
    WsControllerDescriptor, WsDispatchError, WsHandlerFn, WsIdle, WsRespond, WsShutdown,
};

pub use auth::{
    Direct, Injected, IntoAuthenticator, ResolvedAuthenticator, StompAuthFuture,
    StompAuthenticationError, StompAuthenticator, StompConnect, StompPrincipal,
};
pub use body::{Publish, StompOutcome};
pub use broker::{Broker, BrokerExt};
pub use error::{StompBuildError, StompError};
pub use headers::{StompHeaders, StompSession};
// The protocol-generic pub/sub runtime lives in `crate::ws::pubsub`; re-exported here so the STOMP
// serve loop and the crate's historical `ws::stomp::*` surface keep naming them unchanged.
pub use upwell_axum::{ConnectionId, Publisher, SubscriptionRegistry, TopicBus};

static STOMP_HEADERS_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    "__upwell_stomp_headers",
    "StompHeaders",
    TypeDescriptor::of::<StompHeaders>("StompHeaders"),
    WebsocketMessageScope::KEY,
);

static STOMP_SESSION_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    "__upwell_stomp_session",
    "StompSession",
    TypeDescriptor::of::<StompSession>("StompSession"),
    WebsocketMessageScope::KEY,
);

static STOMP_PRINCIPAL_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    "__upwell_stomp_principal",
    "StompPrincipal",
    TypeDescriptor::of::<StompPrincipal>("StompPrincipal"),
    WebsocketMessageScope::KEY,
);

/// STOMP's protocol-specific instantiation of the neutral topic bus.
pub type StompTopicBus = TopicBus<Stomp>;

use broker::{OutFrame, build_message};

/// The outbound-frame channel depth per connection before publishes to a slow consumer are dropped.
const OUTBOUND_BUFFER: usize = 64;

/// Heart-beat and version policy for a [`Stomp`] endpoint.
#[derive(Clone)]
pub struct StompConfig {
    /// If set, the server emits a heart-beat this often when otherwise idle (and advertises it in
    /// `CONNECTED`). `None` disables server heart-beating.
    pub server_heartbeat: Option<Duration>,

    /// The protocol versions the server accepts, highest preference first.
    pub versions: Vec<StompVersion>,

    /// Optional CONNECT authenticator. When present, a connection receives `CONNECTED` only after
    /// it returns a principal; a rejection receives `ERROR` and is closed.
    pub authenticator: Option<Arc<dyn StompAuthenticator>>,
}

impl Default for StompConfig {
    fn default() -> Self {
        Self {
            server_heartbeat: None,
            versions: vec![StompVersion::V1_2, StompVersion::V1_1],
            authenticator: None,
        }
    }
}

impl StompConfig {
    /// Requires successful authentication for this endpoint using `authenticator`.
    ///
    /// Accepts either an async function whose arguments after [`StompConnect`] are injected from the
    /// connection's DI scope, a value that implements [`StompAuthenticator`] directly, or an
    /// [`Injected<T>`] adapter that resolves a component authenticator from the container.
    pub fn with_authenticator<A, M>(mut self, authenticator: A) -> Self
    where
        A: IntoAuthenticator<M>,
    {
        self.authenticator = Some(authenticator.into_authenticator());

        self
    }
}

// The `Stomp` struct is defined in the wasm-safe `crate::stomp` module (so a `#[topics(protocol =
// Stomp)]` set and its client can name it on wasm), with its server-only fields behind a `cfg`. The
// stateful protocol behavior — the `WebsocketProtocol` impl and serve loop — lives here.
use crate::{MESSAGE_ERROR_HEADER, REPLY_SUBSCRIPTION_ID, Stomp, StompBody};

impl PubSubProtocol for Stomp {
    type OutFrame = OutFrame;

    fn frame_message(
        message_id: u64,
        destination: &str,
        sub_id: &str,
        body: &StompBody,
        headers: &[(String, String)],
    ) -> OutFrame {
        build_message(message_id, destination, sub_id, body, headers)
    }
}

impl MessageReply for Stomp {
    fn reply(body: StompBody) -> StompOutcome {
        StompOutcome::Reply(body)
    }
}

impl WebsocketProtocol for Stomp {
    type Payload = StompBody;
    type Outcome = StompOutcome;
    type Options = StompConfig;
    type BuildError = StompBuildError;

    fn build(
        controllers: &[WsControllerDescriptor],
        runtime: &AppRuntime,
        config: StompConfig,
    ) -> Result<Self, Self::BuildError> {
        let mut app_routes: HashMap<&'static str, WsHandlerFn<Stomp>> = HashMap::new();

        for descriptor in controllers {
            for route in descriptor.routes_for::<Stomp>(runtime) {
                app_routes.insert(route.destination, route.handler);
            }
        }

        let bus = runtime
            .root()
            .get::<StompTopicBus>()
            .ok_or(StompBuildError::MissingTopicBus)?;

        Ok(Self {
            app_routes,
            broker: Arc::clone(bus.registry()),
            config,
        })
    }

    async fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: WsConnectionScope,
        mut shutdown: WsShutdown,
    ) {
        let (mut sender, mut receiver) = socket.split();
        let mut idle = WsIdle::from_connection(connection.container());
        // The handshake must come first: read one frame and expect CONNECT/STOMP. A non-CONNECT
        // opener (or a parse failure) is a protocol violation — reply ERROR and abandon the socket.
        let opener = loop {
            tokio::select! {
                _ = shutdown.wait() => return,

                _ = idle.wait() => {
                    if idle.on_timeout()
                        || !matches!(
                            tokio::time::timeout(
                                SOCKET_SEND_TIMEOUT,
                                sender.send(Message::Ping(bytes::Bytes::new())),
                            ).await,
                            Ok(Ok(()))
                        )
                    {
                        return;
                    }
                }

                inbound = receiver.next() => {
                    idle.on_activity();

                    break inbound;
                }
            }
        };

        let (negotiated, principal) = match opener {
            Some(Ok(message)) => match self.negotiate(message, &connection).await {
                Ok(handshake) => handshake,

                Err(error) => {
                    let _ = tokio::time::timeout(
                        SOCKET_SEND_TIMEOUT,
                        sender.send(to_message(error_frame(&error))),
                    )
                    .await;

                    return;
                }
            },

            _ => return,
        };

        // A rejected connection never enters the broker registry.
        let conn_id = self.broker.register();

        let (tx, rx) = mpsc::channel::<OutFrame>(OUTBOUND_BUFFER);
        let mut writer = tokio::spawn(writer_task(sender, rx, self.config.server_heartbeat));

        // CONNECTED confirms the negotiated version (and advertises the server heart-beat).
        let _ = tx
            .send(OutFrame::Frame(connected_frame(negotiated, &self.config)))
            .await;

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    let _ = tx.try_send(OutFrame::Frame(error_frame(&StompError::Frame(
                        "server shutting down".to_owned(),
                    ))));

                    break;
                }

                _ = idle.wait() => {
                    if idle.on_timeout() {
                        tracing::debug!(target: "upwell::axum", "STOMP peer did not answer idle probe");

                        break;
                    }

                    if tx.try_send(OutFrame::Ping).is_err() {
                        break;
                    }
                }

                inbound = receiver.next() => {
                    idle.on_activity();

                    match inbound {
                        // A bare newline (or empty frame) is a client heart-beat, not a STOMP frame;
                        // consume it silently rather than failing to parse it (real clients such as
                        // stomp.js send these).
                        Some(Ok(Message::Text(text))) if is_heartbeat(text.as_bytes()) => {}

                        Some(Ok(Message::Text(text))) => {
                            if self.dispatch(text.as_bytes().to_vec(), &tx, conn_id, &connection, &principal).await.is_break() {
                                break;
                            }
                        }

                        Some(Ok(Message::Binary(bytes))) if is_heartbeat(&bytes) => {}

                        Some(Ok(Message::Binary(bytes))) => {
                            if self.dispatch(bytes.to_vec(), &tx, conn_id, &connection, &principal).await.is_break() {
                                break;
                            }
                        }

                        Some(Ok(Message::Close(_))) | None => break,

                        // Ping/Pong are handled by axum; nothing else is meaningful here.
                        Some(Ok(_)) => {}

                        Some(Err(error)) => {
                            tracing::debug!(target: "upwell::axum", %error, "STOMP connection read error");

                            break;
                        }
                    }
                }
            }
        }

        self.broker.unregister(conn_id);
        drop(tx);

        if tokio::time::timeout(SOCKET_SEND_TIMEOUT, &mut writer)
            .await
            .is_err()
        {
            writer.abort();
            let _ = writer.await;
        }
    }

    fn register(registry: &mut upwell_axum::AppRegistry) {
        upwell_axum::register_topic_bus::<Self>(registry);
        registry.components.extend([
            STOMP_HEADERS_DESCRIPTOR,
            STOMP_SESSION_DESCRIPTOR,
            STOMP_PRINCIPAL_DESCRIPTOR,
        ]);
    }
}

impl Stomp {
    /// The shared broker, so app code outside a handler can publish too.
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Negotiates the protocol version from a CONNECT/STOMP frame, erroring on a non-CONNECT opener
    /// or when no offered version is supported.
    async fn negotiate(
        &self,
        message: Message,
        connection: &WsConnectionScope,
    ) -> Result<(StompVersion, StompPrincipal), StompError> {
        let bytes = match message {
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Binary(bytes) => bytes.to_vec(),

            _ => return Err(StompError::Frame("expected a CONNECT frame".to_owned())),
        };

        // STOMP marks `host` mandatory on CONNECT, but many clients (notably stomp.js) omit it and
        // most brokers tolerate that. `stomp-parser` is strict, so inject a synthetic host when one
        // is absent — `host` is informational and this server ignores it (it reads only the
        // negotiated version).
        let bytes = ensure_connect_host(bytes);

        let frame =
            ClientFrame::try_from(bytes).map_err(|e| StompError::Frame(e.message().to_owned()))?;

        let ClientFrame::Connect(connect) = frame else {
            return Err(StompError::UnexpectedCommand(
                "expected CONNECT before any other frame".to_owned(),
            ));
        };

        let offered: &StompVersions = connect.accept_version().value();

        let version = self
            .config
            .versions
            .iter()
            .find(|candidate| offered.iter().any(|v| v == *candidate))
            .cloned()
            .ok_or_else(|| StompError::VersionMismatch {
                offered: offered.to_string(),
            })?;

        let connect = connect_metadata(&connect);
        let principal = match &self.config.authenticator {
            Some(authenticator) => {
                Arc::clone(authenticator)
                    .authenticate(connect, Arc::clone(connection.container()))
                    .await?
            }

            None => StompPrincipal::anonymous(),
        };

        Ok((version, principal))
    }

    /// Parses and routes one inbound frame. Returns [`ControlFlow::Break`] when the connection
    /// should close (DISCONNECT or a fatal protocol error).
    async fn dispatch(
        &self,
        bytes: Vec<u8>,
        tx: &mpsc::Sender<OutFrame>,
        conn_id: ConnectionId,
        connection: &WsConnectionScope,
        principal: &StompPrincipal,
    ) -> std::ops::ControlFlow<()> {
        use std::ops::ControlFlow::{Break, Continue};

        let frame = match ClientFrame::try_from(bytes) {
            Ok(frame) => frame,

            Err(error) => {
                let _ = tx
                    .send(OutFrame::Frame(error_frame(&StompError::Frame(
                        error.message().to_owned(),
                    ))))
                    .await;

                return Break(());
            }
        };

        match frame {
            ClientFrame::Subscribe(sub) => {
                let receipt = sub.receipt().map(|r| r.value().to_owned());

                self.broker.subscribe(
                    conn_id,
                    sub.id().value(),
                    sub.destination().value(),
                    tx.clone(),
                );

                self.send_receipt(tx, receipt).await;

                Continue(())
            }

            ClientFrame::Unsubscribe(unsub) => {
                let receipt = unsub.receipt().map(|r| r.value().to_owned());

                self.broker.unsubscribe(conn_id, unsub.id().value());
                self.send_receipt(tx, receipt).await;

                Continue(())
            }

            ClientFrame::Send(send) => {
                let destination = send.destination().value().to_owned();
                let receipt = send.receipt().map(|r| r.value().to_owned());
                let content_type = send.content_type().map(|c| c.value().to_owned());
                let custom_headers: Vec<(String, String)> = send
                    .custom
                    .iter()
                    .map(|header| {
                        (
                            header.header_name().to_owned(),
                            (*header.value()).to_owned(),
                        )
                    })
                    .collect();
                let body = StompBody {
                    content_type: content_type.clone(),
                    bytes: send
                        .body()
                        .map(bytes::Bytes::copy_from_slice)
                        .unwrap_or_default(),
                };
                // A request carries a `reply-to` destination (and usually a `correlation-id`); the
                // handler's non-unit return is routed back there rather than broadcast.
                let reply = ReplyContext {
                    tx,
                    reply_to: header_value(&custom_headers, "reply-to"),
                    correlation_id: header_value(&custom_headers, "correlation-id"),
                };
                let headers =
                    StompHeaders::new(send_header_seed(&destination, content_type, custom_headers));
                let message = InboundMessage {
                    destination,
                    body,
                    headers,
                };

                if self
                    .route_send(message, conn_id, connection, principal, reply)
                    .await
                {
                    self.send_receipt(tx, receipt).await;
                }

                Continue(())
            }

            ClientFrame::Disconnect(disconnect) => {
                let receipt = Some(disconnect.receipt().value().to_owned());

                self.send_receipt(tx, receipt).await;

                Break(())
            }

            other => {
                let _ = tx
                    .send(OutFrame::Frame(error_frame(
                        &StompError::UnexpectedCommand(format!("{other:?}")),
                    )))
                    .await;

                Break(())
            }
        }
    }

    /// Routes a `SEND`: an `/app/**` destination invokes its handler (seeding the message scope with
    /// the frame headers and a session handle); any other destination is a direct broker publish.
    async fn route_send(
        &self,
        message: InboundMessage,
        conn_id: ConnectionId,
        connection: &WsConnectionScope,
        principal: &StompPrincipal,
        reply: ReplyContext<'_>,
    ) -> bool {
        let InboundMessage {
            destination,
            body,
            headers,
        } = message;

        let Some(handler) = self.app_routes.get(destination.as_str()) else {
            self.broker.publish(&destination, &body, &[]);

            return true;
        };

        let builder = connection
            .message()
            .seed(headers)
            .seed(StompSession::new(Arc::clone(&self.broker), conn_id))
            .seed(principal.clone());
        let opening = connection.open_message(destination.as_str(), builder);

        let scope = match opening.await {
            Ok(scope) => scope.into_container(),

            Err(error) => {
                tracing::error!(target: "upwell::axum", %error, "STOMP message scope build failed");
                self.deliver_error_reply(
                    &WsDispatchError::Inject("message scope construction failed".to_owned()),
                    &reply,
                )
                .await;

                return false;
            }
        };

        match handler(body, scope).await {
            Ok(StompOutcome::Publish(publishes)) => {
                for publish in publishes {
                    self.broker
                        .publish(&publish.destination, &publish.body, &publish.headers);
                }
            }

            Ok(StompOutcome::Reply(reply_body)) => {
                self.deliver_reply(&destination, reply_body, &reply).await;
            }

            Ok(StompOutcome::Nothing) => {}

            Err(error) => {
                tracing::warn!(target: "upwell::axum", %error, dest = %destination, "STOMP handler failed");
                self.deliver_error_reply(&error, &reply).await;
            }
        }

        true
    }

    /// Routes a request handler's reply back to the requester on its own connection: a directed
    /// `MESSAGE` to the frame's `reply-to`, carrying the `correlation-id` so the client demuxes it to
    /// the awaiting call. A request handler that ran without a `reply-to` is a client bug — the reply
    /// has nowhere to go, so it is logged and dropped.
    async fn deliver_reply(&self, destination: &str, body: StompBody, reply: &ReplyContext<'_>) {
        let Some(reply_to) = &reply.reply_to else {
            tracing::warn!(
                target: "upwell::axum",
                dest = destination,
                "STOMP request handler returned a reply but the frame carried no `reply-to`; dropping"
            );

            return;
        };

        let extra_headers = reply.correlation_headers(&[]);

        let message_id = self.broker.next_message_id();
        let frame = build_message(
            message_id,
            reply_to,
            REPLY_SUBSCRIPTION_ID,
            &body,
            &extra_headers,
        );

        let _ = reply.tx.send(frame).await;
    }

    /// Routes a request handler's *error* back to the requester as a directed `MESSAGE` marked with
    /// the error header (and the `correlation-id`), so the client's awaiting call resolves `Err`
    /// rather than hanging on a reply that will never come. A plain fire-and-forget `SEND` (no
    /// `reply-to`) has no awaiting caller, so the error is only logged (by the caller) and nothing is
    /// sent here.
    async fn deliver_error_reply(&self, error: &WsDispatchError, reply: &ReplyContext<'_>) {
        let Some(reply_to) = &reply.reply_to else {
            return;
        };

        // Send only the stable public category to the peer; the detailed error was already logged
        // by the caller (`STOMP handler failed`), so internal wiring never crosses the wire.
        let body = StompBody {
            content_type: Some("text/plain".to_owned()),
            bytes: bytes::Bytes::copy_from_slice(error.public_message().as_bytes()),
        };
        let extra_headers =
            reply.correlation_headers(&[(MESSAGE_ERROR_HEADER.to_owned(), "1".to_owned())]);

        let message_id = self.broker.next_message_id();
        let frame = build_message(
            message_id,
            reply_to,
            REPLY_SUBSCRIPTION_ID,
            &body,
            &extra_headers,
        );

        let _ = reply.tx.send(frame).await;
    }

    /// Sends a `RECEIPT` for `receipt_id`, if the client requested one.
    async fn send_receipt(&self, tx: &mpsc::Sender<OutFrame>, receipt_id: Option<String>) {
        if let Some(id) = receipt_id {
            let frame: Vec<u8> = ReceiptFrameBuilder::new(id).build().into();
            let _ = tx.send(OutFrame::Frame(frame)).await;
        }
    }
}

/// What a STOMP `#[message]` handler may return, turned into a [`StompOutcome`]. Keeps
/// [`WsRespond`] a single blanket impl (avoiding overlap) while accepting `()`, an explicit
/// outcome, one or many [`Publish`]es, or a `Result` of any of those.
pub trait IntoStompOutcome {
    /// Converts this handler return value into a protocol outcome.
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError>;
}

impl IntoStompOutcome for () {
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError> {
        Ok(StompOutcome::Nothing)
    }
}

impl IntoStompOutcome for StompOutcome {
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError> {
        Ok(self)
    }
}

impl IntoStompOutcome for Publish {
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError> {
        Ok(StompOutcome::Publish(vec![self]))
    }
}

impl IntoStompOutcome for Vec<Publish> {
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError> {
        Ok(StompOutcome::Publish(self))
    }
}

impl<T, E> IntoStompOutcome for Result<T, E>
where
    T: IntoStompOutcome,
{
    fn into_outcome(self) -> Result<StompOutcome, WsDispatchError> {
        self.map_err(|_| WsDispatchError::Application)?
            .into_outcome()
    }
}

impl<R> WsRespond<R> for Stomp
where
    R: IntoStompOutcome,
{
    fn respond(response: R) -> Result<StompOutcome, WsDispatchError> {
        response.into_outcome()
    }
}

/// The per-connection writer task: drains queued frames to the socket and emits server heart-beats
/// when idle. Owns the socket's write half so any connection's publish reaches this socket without
/// touching the reader loop.
async fn writer_task(
    mut sender: futures::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<OutFrame>,
    heartbeat: Option<Duration>,
) {
    let mut ticker = heartbeat.map(tokio::time::interval);

    if let Some(ticker) = ticker.as_mut() {
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    }

    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Some(OutFrame::Frame(bytes)) => {
                    if sender.send(to_message(bytes)).await.is_err() {
                        break;
                    }
                }

                Some(OutFrame::Heartbeat) => {
                    if sender.send(Message::text("\n")).await.is_err() {
                        break;
                    }
                }

                Some(OutFrame::Ping) => {
                    if sender.send(Message::Ping(bytes::Bytes::new())).await.is_err() {
                        break;
                    }
                }

                None => break,
            },

            _ = tick(ticker.as_mut()) => {
                if sender.send(Message::text("\n")).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Resolves on the next heart-beat tick, or never when heart-beating is disabled — so the
/// `select!` arm is inert without a timer.
async fn tick(ticker: Option<&mut tokio::time::Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }

        None => std::future::pending::<()>().await,
    }
}

/// Whether an inbound frame is a client heart-beat — an empty payload or a bare newline
/// (`\n` / `\r\n`) — rather than a STOMP frame to parse.
fn is_heartbeat(bytes: &[u8]) -> bool {
    bytes.is_empty() || bytes == b"\n" || bytes == b"\r\n"
}

/// Injects a `host:upwell` header into a `CONNECT`/`STOMP` frame that lacks one, so a client
/// that omits the (spec-mandatory but widely-skipped) `host` header still connects. Leaves any
/// other frame — and a CONNECT that already has a host — untouched.
fn ensure_connect_host(bytes: Vec<u8>) -> Vec<u8> {
    let is_connect = bytes.starts_with(b"CONNECT") || bytes.starts_with(b"STOMP");

    if !is_connect || contains_subsequence(&bytes, b"\nhost:") {
        return bytes;
    }

    let Some(newline) = bytes.iter().position(|&b| b == b'\n') else {
        return bytes;
    };

    let mut out = Vec::with_capacity(bytes.len() + 16);

    out.extend_from_slice(&bytes[..=newline]);
    out.extend_from_slice(b"host:upwell\n");
    out.extend_from_slice(&bytes[newline + 1..]);

    out
}

/// Whether `haystack` contains `needle` as a contiguous subsequence.
fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Wraps serialized frame bytes in a WebSocket message: text when valid UTF-8 (the common case for
/// STOMP's text protocol), binary otherwise (a binary body).
fn to_message(bytes: Vec<u8>) -> Message {
    match String::from_utf8(bytes) {
        Ok(text) => Message::text(text),

        Err(error) => Message::binary(error.into_bytes()),
    }
}

/// Serializes a `CONNECTED` frame confirming the negotiated version.
fn connected_frame(version: StompVersion, _config: &StompConfig) -> Vec<u8> {
    ConnectedFrameBuilder::new(version).build().into()
}

/// Serializes an `ERROR` frame carrying the error's message.
fn error_frame(error: &StompError) -> Vec<u8> {
    ErrorFrame::from_message(&error.to_string()).into()
}

/// A decoded inbound `SEND`: the destination it targets, its body, and the header set seeded into
/// the message scope for a handler. Groups the three frame-derived inputs `route_send` consumes.
struct InboundMessage {
    destination: String,
    body: StompBody,
    headers: StompHeaders,
}

/// Where a request handler's outcome (a reply or an error) is routed: the requester's own writer
/// `tx`, plus the `reply-to` destination and `correlation-id` it supplied. A plain fire-and-forget
/// `SEND` leaves `reply_to`/`correlation_id` `None`, so nothing is routed back.
struct ReplyContext<'a> {
    tx: &'a mpsc::Sender<OutFrame>,
    reply_to: Option<String>,
    correlation_id: Option<String>,
}

impl ReplyContext<'_> {
    /// The reply's extra headers: `extra` (e.g. the error marker) plus the `correlation-id`, when
    /// the request carried one, so the client demuxes the reply to the awaiting call.
    fn correlation_headers(&self, extra: &[(String, String)]) -> Vec<(String, String)> {
        let mut headers = extra.to_vec();

        if let Some(id) = &self.correlation_id {
            headers.push(("correlation-id".to_owned(), id.clone()));
        }

        headers
    }
}

/// The first value of a custom header by (case-sensitive) name, cloned out of the parsed list.
fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _)| header_name == name)
        .map(|(_, value)| value.clone())
}

/// Builds the `StompHeaders` seed for an app `SEND`: `destination`, then `content-type` if
/// present, then every other header the client sent (in wire order) so a handler injecting
/// `StompHeaders` sees the triggering frame's full header set, not just the two typed ones.
fn send_header_seed(
    destination: &str,
    content_type: Option<String>,
    custom_headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut header_list = vec![("destination".to_owned(), destination.to_owned())];

    if let Some(ct) = content_type {
        header_list.push(("content-type".to_owned(), ct));
    }

    header_list.extend(custom_headers);

    header_list
}

/// Copies the authentication-relevant CONNECT fields out of the parser's borrowing frame.
fn connect_metadata(connect: &stomp_parser::client::ConnectFrame<'_>) -> StompConnect {
    let headers = connect
        .custom
        .iter()
        .map(|header| {
            (
                header.header_name().to_owned(),
                (*header.value()).to_owned(),
            )
        })
        .collect();

    StompConnect::new(
        connect.host().value().to_owned(),
        connect.login().map(|value| value.value().to_owned()),
        connect.passcode().map(|value| value.value().to_owned()),
        headers,
    )
}
