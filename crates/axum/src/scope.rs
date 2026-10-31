//! The axum protocol's component scopes.
//!
//! Plain HTTP and WebSocket traffic are separate topology branches. Each HTTP request opens an
//! [`HttpRequest`] scope directly below the application root. With WebSocket support enabled, each
//! upgraded socket opens one [`WebsocketConnection`] scope below the root and each inbound
//! application message opens a [`WebsocketMessage`] child below that connection.

use upwell_app::{ScopeBoundary, ScopeParent, ScopeTopology};
#[cfg(feature = "ws")]
use upwell_core::ScopeGenerations;
use upwell_core::{Scope, ScopeId};

/// One inbound HTTP request.
pub struct HttpRequest;

/// One live upgraded WebSocket connection.
#[cfg(feature = "ws")]
pub struct WebsocketConnection;

/// One inbound application message on a WebSocket connection.
#[cfg(feature = "ws")]
pub struct WebsocketMessage;

impl Scope for HttpRequest {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "upwell/axum-http-request");
    const RANK: u8 = 100;
    const NAME: &'static str = "HttpRequest";
}

#[cfg(feature = "ws")]
impl Scope for WebsocketConnection {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "upwell/axum-websocket-connection");
    const RANK: u8 = 200;
    const NAME: &'static str = "WebsocketConnection";
    const GENERATIONS: ScopeGenerations = ScopeGenerations::Durable;
}

#[cfg(feature = "ws")]
impl Scope for WebsocketMessage {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "upwell/axum-websocket-message");
    const RANK: u8 = 100;
    const NAME: &'static str = "WebsocketMessage";
}

#[cfg(not(feature = "ws"))]
static AXUM_SCOPE_BOUNDARIES: [ScopeBoundary; 1] =
    [ScopeBoundary::new::<HttpRequest>(&[ScopeParent::Root])];

#[cfg(feature = "ws")]
static AXUM_SCOPE_BOUNDARIES: [ScopeBoundary; 3] = [
    ScopeBoundary::new::<HttpRequest>(&[ScopeParent::Root]),
    ScopeBoundary::new::<WebsocketConnection>(&[ScopeParent::Root]),
    ScopeBoundary::new::<WebsocketMessage>(&[ScopeParent::of::<WebsocketConnection>()]),
];

/// Axum-owned scope topology for HTTP and optional WebSocket traffic.
pub const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&AXUM_SCOPE_BOUNDARIES);

#[cfg(test)]
mod tests;
