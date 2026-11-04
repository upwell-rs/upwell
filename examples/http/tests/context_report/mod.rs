//! Describes the invocation context a WebSocket handler runs in, for the WebSocket tests.

use upwell::Scope;
use upwell::axum::{WebsocketConnection, WebsocketMessage};
use upwell_di::context::{Invocation, InvocationContext};

/// `<kind>:<scope>` for the active context: the invocation kind (`message`, `connection` or
/// `other`) and the innermost scope instance (`message`, `connection` or `other`), or `none`
/// outside every context.
pub fn describe() -> String {
    let Ok(context) = InvocationContext::capture() else {
        return "none".to_owned();
    };

    let kind = match context.invocation() {
        Invocation::WebsocketMessage => "message",
        Invocation::WebsocketConnection => "connection",
        _ => "other",
    };

    let scope = match context.scope_key().map(|key| key.id()) {
        Some(id) if id == WebsocketMessage::ID => "message",
        Some(id) if id == WebsocketConnection::ID => "connection",
        _ => "other",
    };

    format!("{kind}:{scope}")
}
