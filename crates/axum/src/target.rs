//! The invocation targets the HTTP protocol establishes contexts for.

use upwell_di::context::InvocationTarget;

/// The HTTP protocol entry point an invocation context was established for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpTarget {
    /// One HTTP request served by the protocol's router, middleware included.
    Request,
    /// One WebSocket connection, between its upgrade and its close.
    WebsocketConnection,
    /// One message dispatched on a WebSocket connection.
    WebsocketMessage,
}

impl InvocationTarget for HttpTarget {}
