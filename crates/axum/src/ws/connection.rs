//! One upgraded socket's connection scope, handed to its [`WebsocketProtocol`](super::WebsocketProtocol).

use std::sync::Arc;

use upwell_app::{OpenScope, ScopeBuilder, ScopeOpener};
use upwell_di::ScopeContainer;

use crate::scope::{WebsocketConnection, WebsocketMessage};

/// One upgraded socket's [`WebsocketConnection`] scope and the prepared opener of the
/// [`WebsocketMessage`] scopes a protocol opens under it.
pub struct WsConnectionScope {
    scope: OpenScope<WebsocketConnection>,
    messages: ScopeOpener<WebsocketMessage>,
}

impl WsConnectionScope {
    pub(crate) fn new(
        scope: OpenScope<WebsocketConnection>,
        messages: ScopeOpener<WebsocketMessage>,
    ) -> Self {
        Self { scope, messages }
    }

    /// The connection scope's container.
    pub fn container(&self) -> &Arc<ScopeContainer> {
        self.scope.container()
    }

    /// Starts a message scope under this connection, on the connection's runtime generation.
    pub fn message(&self) -> ScopeBuilder<'_, WebsocketMessage, WebsocketConnection> {
        self.messages.builder().under(&self.scope)
    }
}
