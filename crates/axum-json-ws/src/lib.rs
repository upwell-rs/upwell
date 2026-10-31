//! JSON-envelope request/reply over Upwell's neutral WebSocket host.
//!
//! Each inbound text frame is a `{ "dest": "<destination>", "id": <n>, "payload": <json> }` call;
//! each reply is a `{ "dest": <destination>, "id": <n>, "ok": <json> }` or
//! `{ "dest": <destination>, "id": <n>, "error": "<message>" }` frame, echoing the request `id` so a
//! client can correlate replies. This maps the existing request/response handler shape onto a
//! socket; a future STOMP protocol would extend the [`WebsocketProtocol`] seam with
//! subscription/broadcast (server-initiated `MESSAGE` fan-out) without changing it.

#[cfg(not(target_family = "wasm"))]
use std::collections::HashMap;
#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(not(target_family = "wasm"))]
use tracing::{debug, warn};
#[cfg(not(target_family = "wasm"))]
use upwell_axum::AppRuntime;
#[cfg(not(target_family = "wasm"))]
use upwell_axum::axum::body::Bytes;
#[cfg(not(target_family = "wasm"))]
use upwell_axum::axum::extract::ws::{Message, Utf8Bytes, WebSocket};
#[cfg(feature = "client")]
use upwell_client::{ClientError, ErrorBody};
use upwell_transport::CodecError;

#[cfg(feature = "client")]
use upwell_axum::MessagingClientProtocol;
#[cfg(feature = "client")]
use upwell_axum::client::{
    MessageRequest, MessageSend, TokioTungsteniteWs, WebsocketClient, WebsocketClientProtocol,
    WebsocketDecodes, WebsocketEncodes, WsClientFrame,
};
#[cfg(not(target_family = "wasm"))]
use upwell_axum::{
    MessageReply, SOCKET_SEND_TIMEOUT, WebsocketProtocol, WsConnectionScope,
    WsControllerDescriptor, WsDispatchError, WsHandlerFn, WsIdle, WsRespond, WsShutdown,
};
use upwell_axum::{MessagingProtocol, TopicCodec};

/// The JSON value carried as a message body.
pub type WsValue = serde_json::Value;

/// The baseline JSON-envelope protocol: a flat destination → handler table, point-to-point
/// request/response over one socket. Each message opens a fresh
/// [`WebsocketMessage`](upwell_axum::WebsocketMessage) scope for handler DI.
pub struct JsonWs {
    #[cfg(not(target_family = "wasm"))]
    routes: HashMap<&'static str, WsHandlerFn<Self>>,
}

/// [`JsonWs`]'s handler outcome: no reply for send mode or one encoded request reply.
#[cfg(not(target_family = "wasm"))]
pub struct WsReply(Option<WsValue>);

/// JSON body codec used by generated message clients and handlers for [`JsonWs`].
pub struct JsonWsCodec;

/// Remote error status returned by a JSON WebSocket peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonWsStatus {
    /// The peer returned an application or protocol error envelope.
    Error,
}

/// An inbound call frame.
#[derive(Deserialize, Serialize)]
struct Inbound {
    dest: String,

    #[serde(default)]
    id: Option<u64>,

    #[serde(default)]
    payload: WsValue,
}

/// An outbound reply frame: exactly one of `ok` / `error` is set.
#[derive(Serialize)]
#[cfg(not(target_family = "wasm"))]
struct Outbound<'a> {
    dest: &'a str,

    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<WsValue>,

    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[cfg(feature = "client")]
#[derive(Deserialize)]
struct ClientOutbound {
    id: Option<u64>,

    #[serde(default)]
    ok: Option<WsValue>,

    #[serde(default)]
    error: Option<String>,
}

#[cfg(not(target_family = "wasm"))]
impl WebsocketProtocol for JsonWs {
    type Payload = WsValue;
    type Outcome = WsReply;
    type Options = ();
    type BuildError = std::convert::Infallible;

    fn build(
        controllers: &[WsControllerDescriptor],
        runtime: &AppRuntime,
        _options: (),
    ) -> Result<Self, Self::BuildError> {
        let mut routes: HashMap<&'static str, WsHandlerFn<Self>> = HashMap::new();

        for descriptor in controllers {
            for route in descriptor.routes_for::<Self>(runtime) {
                routes.insert(route.destination, route.handler);
            }
        }

        Ok(Self { routes })
    }

    async fn serve(
        self: Arc<Self>,
        mut socket: WebSocket,
        connection: WsConnectionScope,
        mut shutdown: WsShutdown,
    ) {
        let mut idle = WsIdle::from_connection(connection.container());

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    let _ = tokio::time::timeout(
                        SOCKET_SEND_TIMEOUT,
                        socket.send(Message::Close(None)),
                    ).await;

                    break;
                }

                _ = idle.wait() => {
                    if idle.on_timeout() {
                        debug!(target: "upwell::axum", "ws peer did not answer idle probe");

                        break;
                    }

                    if !matches!(
                        tokio::time::timeout(
                            SOCKET_SEND_TIMEOUT,
                            socket.send(Message::Ping(Bytes::new())),
                        ).await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }

                inbound = socket.recv() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            idle.on_activity();
                            let reply = self.handle_text(text.as_str(), &connection).await;

                            if let Some(reply) = reply
                                && !matches!(
                                    tokio::time::timeout(
                                        SOCKET_SEND_TIMEOUT,
                                        socket.send(Message::Text(Utf8Bytes::from(reply))),
                                    ).await,
                                    Ok(Ok(()))
                                )
                            {
                                break;
                            }
                        }

                        Some(Ok(Message::Close(_))) | None => break,

                        // Binary/ping/pong aren't application messages, but they prove the peer is
                        // still consuming the connection and reset its idle probe.
                        Some(Ok(_)) => idle.on_activity(),

                        Some(Err(error)) => {
                            debug!(target: "upwell::axum", %error, "ws connection read error");

                            break;
                        }
                    }
                }
            }
        }
    }
}

impl MessagingProtocol for JsonWs {
    type Body = WsValue;
    type DefaultCodec = JsonWsCodec;
}

impl TopicCodec<JsonWs> for JsonWsCodec {
    fn encode<T: Serialize>(value: &T) -> Result<WsValue, CodecError> {
        serde_json::to_value(value).map_err(|error| CodecError::internal(error.to_string()))
    }

    fn decode<T: DeserializeOwned>(body: WsValue) -> Result<T, CodecError> {
        serde_json::from_value(body).map_err(|error| CodecError::bad_input(error.to_string()))
    }
}

#[cfg(not(target_family = "wasm"))]
impl MessageReply for JsonWs {
    fn reply(body: WsValue) -> WsReply {
        WsReply(Some(body))
    }
}

#[cfg(not(target_family = "wasm"))]
impl JsonWs {
    /// Routes one inbound text frame and renders its reply. Returns `None` for a frame that can't be
    /// parsed at all (no `dest` to correlate a reply against) — it is dropped with a warning.
    ///
    /// Opens a fresh per-message [`WebsocketMessage`](upwell_axum::WebsocketMessage) scope
    /// parented at the socket's connection scope, so a handler's `Inject<T>` resolves
    /// message-scoped components per message
    /// (and connection-/singleton-scoped ones through the chain).
    async fn handle_text(&self, text: &str, connection: &WsConnectionScope) -> Option<String> {
        let inbound: Inbound = match serde_json::from_str(text) {
            Ok(inbound) => inbound,

            Err(error) => {
                warn!(target: "upwell::axum", %error, "unparseable ws frame; dropping");

                return None;
            }
        };

        let result = match self.routes.get(inbound.dest.as_str()) {
            Some(handler) => match connection
                .open_message(inbound.dest.as_str(), connection.message())
                .await
            {
                Ok(scope) => handler(inbound.payload, scope).await,

                Err(error) => Err(WsDispatchError::Inject(error.to_string())),
            },

            None => Err(WsDispatchError::NotFound(inbound.dest.clone())),
        };

        render_reply(&inbound.dest, inbound.id, result)
    }
}

#[cfg(not(target_family = "wasm"))]
impl<R> WsRespond<R> for JsonWs
where
    R: JsonWsSendOutcome,
{
    fn respond(response: R) -> Result<WsReply, WsDispatchError> {
        response.into_outcome()
    }
}

/// A value accepted from a JSON WebSocket send-mode handler.
#[cfg(not(target_family = "wasm"))]
pub trait JsonWsSendOutcome {
    /// Converts the value to a send outcome. Send success has no reply body.
    fn into_outcome(self) -> Result<WsReply, WsDispatchError>;
}

#[cfg(not(target_family = "wasm"))]
impl<T> JsonWsSendOutcome for T {
    fn into_outcome(self) -> Result<WsReply, WsDispatchError> {
        Ok(WsReply(None))
    }
}

#[cfg(feature = "client")]
impl WebsocketClientProtocol for JsonWs {
    type Key = u64;
    type Status = JsonWsStatus;
    type Payload = WsValue;

    fn next_key(counter: u64) -> Self::Key {
        counter
    }

    fn encode_call<T>(
        destination: &str,
        key: &Self::Key,
        payload: T,
    ) -> Result<WsClientFrame, CodecError>
    where
        Self: WebsocketEncodes<T>,
    {
        let inbound = Inbound {
            dest: destination.to_string(),
            id: Some(*key),
            payload: <Self as WebsocketEncodes<T>>::encode_payload(payload)?,
        };

        serde_json::to_string(&inbound)
            .map(WsClientFrame::Text)
            .map_err(|e| CodecError::internal(e.to_string()))
    }

    fn encode_send<T>(destination: &str, payload: T) -> Result<WsClientFrame, CodecError>
    where
        Self: WebsocketEncodes<T>,
    {
        let inbound = Inbound {
            dest: destination.to_string(),
            id: None,
            payload: <Self as WebsocketEncodes<T>>::encode_payload(payload)?,
        };

        serde_json::to_string(&inbound)
            .map(WsClientFrame::Text)
            .map_err(|error| CodecError::internal(error.to_string()))
    }

    fn reply_key(frame: &WsClientFrame) -> Result<Option<Self::Key>, CodecError> {
        let WsClientFrame::Text(frame) = frame else {
            return Err(CodecError::bad_input("JSON WebSocket reply was binary"));
        };
        let outbound: ClientOutbound =
            serde_json::from_str(frame).map_err(|e| CodecError::bad_input(e.to_string()))?;

        Ok(outbound.id)
    }

    fn decode_reply<T>(frame: WsClientFrame) -> Result<T, ClientError<JsonWsStatus>>
    where
        Self: WebsocketDecodes<T>,
    {
        let WsClientFrame::Text(frame) = frame else {
            return Err(ClientError::Decode(
                "JSON WebSocket reply was binary".to_owned(),
            ));
        };
        let outbound: ClientOutbound =
            serde_json::from_str(&frame).map_err(|e| ClientError::Decode(e.to_string()))?;

        if let Some(error) = outbound.error {
            return Err(ClientError::Remote(ErrorBody::new(
                JsonWsStatus::Error,
                error.into_bytes(),
            )));
        }

        let ok = outbound
            .ok
            .ok_or_else(|| ClientError::Decode("ws reply contained neither ok nor error".into()))?;

        <Self as WebsocketDecodes<T>>::decode_payload(ok)
            .map_err(|e| ClientError::Decode(e.to_string()))
    }
}

#[cfg(feature = "client")]
impl MessagingClientProtocol for JsonWs {
    type Status = JsonWsStatus;
}

#[cfg(all(feature = "client", feature = "tungstenite"))]
impl MessageRequest<JsonWs> for TokioTungsteniteWs<JsonWs> {
    async fn request(
        &self,
        destination: &str,
        body: WsValue,
    ) -> Result<WsValue, ClientError<JsonWsStatus>> {
        WebsocketClient::<JsonWs, WsValue, WsValue>::websocket_call(self, destination, body).await
    }
}

#[cfg(all(feature = "client", feature = "tungstenite"))]
impl MessageSend<JsonWs> for TokioTungsteniteWs<JsonWs> {
    async fn send(
        &self,
        destination: &str,
        body: WsValue,
    ) -> Result<(), ClientError<JsonWsStatus>> {
        self.send_message(destination, body).await
    }
}

/// Connects JSON WebSocket in a browser and attaches it to the shared connection.
#[cfg(all(target_family = "wasm", feature = "tungstenite"))]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = connectJsonWs)]
pub async fn connect_json_ws(
    connection: &upwell_axum::client::Connection,
    endpoint: String,
) -> Result<(), wasm_bindgen::JsError> {
    let url = connection.websocket_url(&endpoint);
    let transport = TokioTungsteniteWs::<JsonWs>::connect(url)
        .await
        .map_err(|error| wasm_bindgen::JsError::new(&error.to_string()))?;

    connection.attach_transport::<JsonWs, _>(transport);

    Ok(())
}

/// Detaches JSON WebSocket from the shared browser connection.
#[cfg(all(target_family = "wasm", feature = "tungstenite"))]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = disconnectJsonWs)]
pub fn disconnect_json_ws(connection: &upwell_axum::client::Connection) {
    let _ = connection.detach_transport::<JsonWs, TokioTungsteniteWs<JsonWs>>();
}

#[cfg(all(target_family = "wasm", feature = "tungstenite"))]
impl upwell_axum::client::TopicWasmClient for JsonWs {
    type Transport = TokioTungsteniteWs<JsonWs>;

    fn transport(
        connection: &upwell_axum::client::Connection,
    ) -> Result<Self::Transport, wasm_bindgen::JsError> {
        connection.transport::<JsonWs, Self::Transport>()
    }
}

#[cfg(feature = "client")]
impl<T> WebsocketEncodes<T> for JsonWs
where
    T: Serialize,
{
    fn encode_payload(value: T) -> Result<<Self as WebsocketClientProtocol>::Payload, CodecError> {
        serde_json::to_value(value).map_err(|e| CodecError::bad_input(e.to_string()))
    }
}

#[cfg(feature = "client")]
impl<T> WebsocketDecodes<T> for JsonWs
where
    T: DeserializeOwned,
{
    fn decode_payload(value: <Self as WebsocketClientProtocol>::Payload) -> Result<T, CodecError> {
        serde_json::from_value(value).map_err(|e| CodecError::bad_input(e.to_string()))
    }
}

/// Renders a dispatch result into the JSON reply frame (correlating the request `id`): a `{ok}`
/// frame on success, a `{error}` frame on failure. Pure (no scope), so the framing is unit-testable.
#[cfg(not(target_family = "wasm"))]
fn render_reply(
    dest: &str,
    id: Option<u64>,
    result: Result<WsReply, WsDispatchError>,
) -> Option<String> {
    let outbound = match result {
        Ok(WsReply(None)) => return None,

        Ok(WsReply(Some(value))) => Outbound {
            dest,
            id,
            ok: Some(value),
            error: None,
        },

        Err(error) => {
            warn!(
                target: "upwell::axum",
                %error,
                dest,
                "ws message dispatch failed"
            );

            id?;

            Outbound {
                dest,
                id,
                ok: None,
                error: Some(error.public_message().to_owned()),
            }
        }
    };

    serde_json::to_string(&outbound).ok()
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests;
