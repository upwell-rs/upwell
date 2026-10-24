//! Configuration owned and automatically bound by the axum protocol definition.

use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;
use upwell_macros::config;

/// Validates a configured static Axum mount path before router construction.
pub(crate) fn validate_mount_path(label: &str, path: &str) -> crate::Result<()> {
    let invalid_character = path
        .chars()
        .find(|character| matches!(character, '?' | '#' | '{' | '}' | ':' | '*'));

    if path.is_empty() || !path.starts_with('/') {
        return Err(crate::Error::Config(format!(
            "{label} (`{path}`) must be an absolute path beginning with `/`"
        )));
    }

    if let Some(character) = invalid_character {
        return Err(crate::Error::Config(format!(
            "{label} (`{path}`) must be static and cannot contain `{character}`"
        )));
    }

    if path.contains("//") {
        return Err(crate::Error::Config(format!(
            "{label} (`{path}`) cannot contain empty path segments"
        )));
    }

    Ok(())
}

/// The property path the [`Axum`](crate::Axum) always binds.
pub const AXUM_CONFIG_PATH: &str = "axum";

/// Listener settings for the axum HTTP server.
///
/// The protocol definition always binds this type at [`AXUM_CONFIG_PATH`], even when application config
/// auto-discovery is disabled. All fields have environment-aware defaults, so an axum app can be
/// built and served without declaring configuration of its own:
///
/// ```toml
/// [axum]
/// bind = "0.0.0.0"
/// port = 8080
/// ```
#[config(upwell = ::upwell_config)]
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AxumConfig {
    /// The IP address on which the HTTP listener accepts connections.
    #[default = "${AXUM_BIND:127.0.0.1}"]
    pub bind: IpAddr,

    /// The TCP port on which the HTTP listener accepts connections.
    #[default = "${AXUM_PORT:3000}"]
    pub port: u16,

    /// A global path prefix every route is served under. Empty (or `"/"`) mounts routes at the
    /// root; `"/api"` mounts the whole application (controllers and any OpenAPI routes) under
    /// `/api`. Surfaced in the OpenAPI document as a `server` URL so documented paths stay relative.
    #[default = "${AXUM_BASE_PATH:}"]
    pub base_path: String,

    /// Maximum request body accepted by Axum's buffered body extractors, in bytes.
    #[default = "${AXUM_MAX_REQUEST_BODY_BYTES:2097152}"]
    pub max_request_body_bytes: usize,

    /// Maximum total bytes accepted by a streamed NDJSON request body. A value of `0` disables
    /// this limit; the per-line limit remains enforced.
    #[default = "${AXUM_MAX_STREAM_REQUEST_BYTES:16777216}"]
    pub max_stream_request_bytes: usize,

    /// Maximum number of decoded items accepted from one streamed NDJSON request. A value of `0`
    /// disables this limit.
    #[default = "${AXUM_MAX_STREAM_REQUEST_ITEMS:100000}"]
    pub max_stream_request_items: usize,

    /// Maximum lifetime of a streamed NDJSON request body, in milliseconds. A value of `0`
    /// disables this stream-specific deadline (the global request timeout can still apply).
    #[default = "${AXUM_STREAM_REQUEST_TIMEOUT_MS:30000}"]
    pub stream_request_timeout_ms: u64,

    /// Maximum WebSocket message size, in bytes. This is applied during the upgrade before any
    /// protocol attempts to decode the message.
    #[default = "${AXUM_MAX_WEBSOCKET_MESSAGE_BYTES:262144}"]
    pub max_websocket_message_bytes: usize,

    /// Maximum individual WebSocket frame size, in bytes. Fragmented messages remain subject to
    /// [`max_websocket_message_bytes`](Self::max_websocket_message_bytes).
    #[default = "${AXUM_MAX_WEBSOCKET_FRAME_BYTES:65536}"]
    pub max_websocket_frame_bytes: usize,

    /// Maximum number of live WebSocket connections across each mounted endpoint. A value of `0`
    /// disables admission limiting for deployments that enforce it upstream.
    #[default = "${AXUM_MAX_WEBSOCKET_CONNECTIONS:1024}"]
    pub max_websocket_connections: usize,

    /// How long a WebSocket may remain silent before the server probes it, in milliseconds. The
    /// peer then has one more interval to answer the ping before the connection is dropped. A value
    /// of `0` disables idle probing.
    #[default = "${AXUM_WEBSOCKET_IDLE_TIMEOUT_MS:60000}"]
    pub websocket_idle_timeout_ms: u64,

    /// Maximum time allowed for routing, middleware, and handler execution, in milliseconds.
    /// A value of `0` disables the request deadline.
    #[default = "${AXUM_REQUEST_TIMEOUT_MS:30000}"]
    pub request_timeout_ms: u64,

    /// Time allowed for in-flight requests to finish after shutdown starts, in milliseconds.
    /// Once elapsed, remaining connections are forcibly dropped. A value of `0` waits forever.
    #[default = "${AXUM_GRACEFUL_SHUTDOWN_TIMEOUT_MS:30000}"]
    pub graceful_shutdown_timeout_ms: u64,
}

impl AxumConfig {
    /// The configured listener address.
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }
}

impl Default for AxumConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::from([127, 0, 0, 1]),
            port: 3000,
            base_path: String::new(),
            max_request_body_bytes: 2 * 1024 * 1024,
            max_stream_request_bytes: 16 * 1024 * 1024,
            max_stream_request_items: 100_000,
            stream_request_timeout_ms: 30_000,
            max_websocket_message_bytes: 256 * 1024,
            max_websocket_frame_bytes: 64 * 1024,
            max_websocket_connections: 1_024,
            websocket_idle_timeout_ms: 60_000,
            request_timeout_ms: 30_000,
            graceful_shutdown_timeout_ms: 30_000,
        }
    }
}

#[cfg(feature = "openapi")]
mod openapi;

#[cfg(feature = "openapi")]
pub use openapi::{AXUM_OPENAPI_CONFIG_PATH, OpenApiConfig, OpenApiUi};

#[cfg(test)]
mod tests;
