//! One upgraded socket's connection scope, handed to its [`WebsocketProtocol`](super::WebsocketProtocol).

use std::collections::HashMap;
use std::sync::Arc;

use upwell_app::{OpenScope, ScopeBuilder, ScopeEntry, ScopeOpener};
use upwell_di::ScopeContainer;

use super::WsControllerDescriptor;
use crate::scope::{WebsocketConnection, WebsocketMessage};

/// The entry of each message handler of one endpoint, by destination.
pub(crate) type MessageEntries = HashMap<&'static str, ScopeEntry<WebsocketMessage>>;

/// One upgraded socket's [`WebsocketConnection`] scope and the prepared opener of the
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

    /// Starts a message scope under this connection, on the connection's runtime generation.
    pub fn message(&self) -> ScopeBuilder<'_, WebsocketMessage, WebsocketConnection> {
        self.messages.builder().under(&self.scope)
    }

    /// Opens a message scope `builder` started for a message to `destination`: with the
    /// destination handler's entry plan, which builds only what the handler injects, or with
    /// every slot of the scope when no handler declares the destination.
    ///
    /// # Errors
    ///
    /// As [`ScopeBuilder::open`].
    pub async fn open_message(
        &self,
        destination: &str,
        builder: ScopeBuilder<'_, WebsocketMessage, WebsocketConnection>,
    ) -> upwell_app::Result<OpenScope<WebsocketMessage>> {
        match self.entries.get(destination) {
            Some(entry) => builder.open(entry).await,
            None => builder.open_default().await,
        }
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
