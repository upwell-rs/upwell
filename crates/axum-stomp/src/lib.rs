//! STOMP's implementation of the protocol-generic messaging wire contract.
//!
//! The generic capability traits ([`MessagingProtocol`](crate::messaging::MessagingProtocol),
//! [`TopicCodec`](crate::messaging::TopicCodec), …) live in [`crate::messaging`] so they compile
//! behind `feature = "ws"` alone. This module holds the STOMP-specific wire pieces — the
//! [`StompBody`], the [`StompCodec`]/[`JsonCodec`], and STOMP's impls of the generic traits — behind
//! `feature = "stomp"`.
//!
//! The [`Stomp`] type itself lives here (not the server-only `ws` module) so a `#[topics(protocol =
//! Stomp)]` set and its generated client can name it on wasm; its server-only state is behind a
//! `cfg`, so on wasm it is a fieldless protocol tag. The stateful server behavior
//! (`WebsocketProtocol`, the broker serve loop) is implemented in [`crate::ws::stomp`]. The
//! server-only fan-out types ([`Publish`](crate::ws::stomp::Publish) /
//! [`StompOutcome`](crate::ws::stomp::StompOutcome)) also stay in that module.

use bytes::Bytes;
use upwell_transport::CodecError;

/// The `subscription` header value the server stamps on a request/response reply `MESSAGE`. It is a
/// sentinel (never a real client subscription id, which are `sub-*`), so the client consults its
/// request-correlation table only for frames actually carrying a reply — a broadcast can never be
/// mistaken for one. Named on both sides (server framing, client demux), so it lives in this
/// target-agnostic module.
pub(crate) const REPLY_SUBSCRIPTION_ID: &str = "reply";

/// The custom header the server sets on a request/response reply `MESSAGE` to mark it an *error*
/// reply (the handler failed). Its presence flips the client's awaiting call from `Ok(body)` to
/// `Err`, so a failing request handler resolves the caller instead of hanging it. Shared by the
/// server (framing) and the client (demux).
pub(crate) const MESSAGE_ERROR_HEADER: &str = "upwell-error";

#[cfg(not(target_family = "wasm"))]
use std::collections::HashMap;
#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
use upwell_axum::WsHandlerFn;

/// A STOMP frame body: opaque bytes with an optional `content-type`. Handlers usually receive a
/// decoded type (via [`WsCodec`](crate::ws::WsCodec) JSON decoding) rather than this directly.
#[derive(Clone, Debug, Default)]
pub struct StompBody {
    /// The `content-type` header value, if the frame carried one.
    pub content_type: Option<String>,

    /// The raw body bytes.
    pub bytes: Bytes,
}

impl StompBody {
    /// A JSON body: the bytes plus `application/json`.
    pub fn json(bytes: impl Into<Bytes>) -> Self {
        Self {
            content_type: Some("application/json".to_owned()),
            bytes: bytes.into(),
        }
    }

    /// Serializes `value` to a JSON body. Used by `#[topics]`-generated
    /// [`Topic::encode`](crate::messaging::Topic::encode) impls.
    pub fn from_serialize<T: serde::Serialize>(value: &T) -> Result<Self, CodecError> {
        let bytes = serde_json::to_vec(value).map_err(|e| CodecError::internal(e.to_string()))?;

        Ok(Self::json(bytes))
    }
}

/// The STOMP protocol tag and (on the server) its stateful pub/sub state.
///
/// On wasm this is a fieldless tag: it names the protocol for a `#[topics(protocol = Stomp)]` set
/// and the generated client's `MessageSend<Stomp>`/`TopicSubscribe<Stomp>` bounds. On the server its
/// `cfg`-gated fields hold the `/app/**` handler table, the shared [`Broker`] and the endpoint
/// config; the [`WebsocketProtocol`](crate::ws::WebsocketProtocol) impl and serve loop live
/// in [`crate::ws::stomp`].
pub struct Stomp {
    /// The `/app/**` destination → `#[message]` handler table.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) app_routes: HashMap<&'static str, WsHandlerFn<Stomp>>,

    /// The shared broker for `/topic/**` fan-out (one per endpoint, cloned from the DI bus).
    #[cfg(not(target_family = "wasm"))]
    pub(crate) broker: Arc<server::Broker>,

    /// Heart-beat/version/authenticator policy for this endpoint.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) config: server::StompConfig,
}

impl upwell_axum::MessagingProtocol for Stomp {
    type Body = StompBody;
    type DefaultCodec = JsonCodec;
}

/// The wire codec for a STOMP topic set's message bodies — how a payload is serialized on the
/// server's publish and deserialized on the client's subscribe. Selected per topic set with
/// `#[topics(codec = ..)]`; defaults to [`JsonCodec`]. Provide your own for a more compact wire
/// format (e.g. a postcard-backed codec) and both directions follow it, since the same codec type
/// is named on both sides. A blanket impl makes every `StompCodec` a
/// [`TopicCodec<Stomp>`](crate::messaging::TopicCodec), so it slots into the protocol-generic topics
/// machinery.
pub trait StompCodec: Send + Sync + 'static {
    /// Serializes a payload into a body.
    fn encode<T: serde::Serialize>(value: &T) -> Result<StompBody, CodecError>;

    /// Deserializes a payload from a body.
    fn decode<T: serde::de::DeserializeOwned>(body: StompBody) -> Result<T, CodecError>;
}

/// The default [`StompCodec`]: JSON bodies (`application/json`).
pub struct JsonCodec;

impl StompCodec for JsonCodec {
    fn encode<T: serde::Serialize>(value: &T) -> Result<StompBody, CodecError> {
        StompBody::from_serialize(value)
    }

    fn decode<T: serde::de::DeserializeOwned>(body: StompBody) -> Result<T, CodecError> {
        serde_json::from_slice(&body.bytes).map_err(|e| CodecError::bad_input(e.to_string()))
    }
}

impl upwell_axum::TopicCodec<Stomp> for JsonCodec {
    fn encode<T: serde::Serialize>(value: &T) -> Result<StompBody, CodecError> {
        <Self as StompCodec>::encode(value)
    }

    fn decode<T: serde::de::DeserializeOwned>(body: StompBody) -> Result<T, CodecError> {
        <Self as StompCodec>::decode(body)
    }
}

#[cfg(not(target_family = "wasm"))]
mod server;

#[cfg(feature = "client")]
mod client;

#[cfg(not(target_family = "wasm"))]
pub use server::*;

#[cfg(feature = "client")]
pub use client::StompStatus;

#[cfg(all(feature = "client", feature = "tungstenite"))]
pub use client::{StompClientTransport, StompConnectOptions};

#[cfg(all(target_family = "wasm", feature = "tungstenite"))]
pub use client::{connect_stomp, disconnect_stomp};
