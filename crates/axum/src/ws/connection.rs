//! One upgraded socket's connection scope, handed to its [`WebsocketProtocol`](super::WebsocketProtocol).
//!
//! A protocol's whole `serve` runs in the connection's invocation context, whose innermost scope
//! is the connection once the protocol opens it, so connection-level code never sees a message
//! scope. Each message's handler
//! runs in a context of its own ([`WsConnectionScope::message_context`]), nested in the
//! connection's and ending with the handler. Work a protocol does between messages stays in the
//! connection's.

use std::collections::HashMap;
use std::sync::Arc;

use upwell_app::{EntryScope, OpenScope, ScopeBuilder, ScopeEntry, ScopeOpener};
use upwell_di::ScopeContainer;
use upwell_di::context::{InvocationContext, TargetRef};

use super::WsControllerDescriptor;
use crate::scope::{WebsocketConnection, WebsocketMessage};
use crate::target::HttpTarget;

/// The entry of each message handler of one endpoint, by destination.
pub(crate) type MessageEntries = HashMap<&'static str, ScopeEntry<WebsocketMessage>>;

/// One upgraded socket's open [`WebsocketConnection`] scope, made by
/// [`PreparedConnection::open`](super::PreparedConnection::open), and the prepared opener of the
/// [`WebsocketMessage`] scopes a protocol opens under it.
pub struct WsConnectionScope {
    scope: OpenScope<WebsocketConnection>,
    messages: ScopeOpener<WebsocketMessage>,
    entries: Arc<MessageEntries>,
}

impl WsConnectionScope {
    pub(crate) fn new(
        scope: OpenScope<WebsocketConnection>,
        messages: ScopeOpener<WebsocketMessage>,
        entries: Arc<MessageEntries>,
    ) -> Self {
        Self {
            scope,
            messages,
            entries,
        }
    }

    /// The connection scope's container.
    pub fn container(&self) -> &Arc<ScopeContainer> {
        self.scope.container()
    }

    /// The invocation context of one message dispatched in `scope`, a message scope this
    /// connection opened. A protocol runs the message's handler in it, so the handler and what
    /// it calls see the message scope, or the connection scope until a deferred message scope
    /// opens.
    #[doc(hidden)]
    pub fn message_context(&self, scope: &EntryScope<WebsocketMessage>) -> InvocationContext {
        scope.establish(
            self.messages
                .context(TargetRef::from_static(&HttpTarget::WebsocketMessage)),
        )
    }

    /// Starts a message scope under this connection, on the connection's runtime generation.
    pub fn message(&self) -> ScopeBuilder<'_, WebsocketMessage, WebsocketConnection> {
        self.messages.builder(WebsocketMessage).under(&self.scope)
    }

    /// Opens a message scope `builder` started for a message to `destination`: with the
    /// destination handler's entry plan, which builds only what the handler injects, or with
    /// every slot of the scope when no handler declares the destination. A handler whose plan
    /// builds nothing gets a scope that opens only when something resolves from it.
    ///
    /// # Errors
    ///
    /// As [`ScopeBuilder::open_when_needed`].
    pub async fn open_message(
        &self,
        destination: &str,
        builder: ScopeBuilder<'_, WebsocketMessage, WebsocketConnection>,
    ) -> upwell_app::Result<EntryScope<WebsocketMessage>> {
        builder
            .open_when_needed(self.entries.get(destination))
            .await
    }
}

/// The entry of every described message handler of `controllers`, by destination.
pub(crate) fn message_entries(
    controllers: &[WsControllerDescriptor],
    opener: &ScopeOpener<WebsocketMessage>,
) -> upwell_app::Result<MessageEntries> {
    let mut entries = MessageEntries::new();

    for route in controllers.iter().flat_map(WsControllerDescriptor::routes) {
        if let Some(message) = route.message() {
            entries.insert(message.destination, opener.entry(message.key)?);
        }
    }

    Ok(entries)
}
