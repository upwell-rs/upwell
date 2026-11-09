//! One upgraded socket whose connection scope the protocol opens when its handshake allows.

use std::any::Any;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, close_code};
use upwell_app::{ScopeOpener, Seeds};
use upwell_di::ScopeContainer;
use upwell_di::context::bind_opened_scope;

use super::connection::MessageEntries;
use super::{WebsocketUpgradeMeta, WsConnectionMeta, WsConnectionScope};
use crate::scope::{WebsocketConnection, WebsocketMessage};

/// An upgraded socket's [`WebsocketConnection`] scope before it opens.
///
/// The framework hands one to [`WebsocketProtocol::serve`](super::WebsocketProtocol::serve)
/// instead of an open scope, so a protocol with an in-band handshake, such as STOMP's
/// `CONNECT`, authenticates first and opens the scope with what authentication produced. A
/// protocol without a handshake opens it at once. It cannot be cloned, and
/// [`open`](Self::open) consumes it, so a connection opens at most once and no
/// connection-scoped component is built before the protocol opens it.
///
/// `serve` runs in the connection's invocation context from the start; until the scope opens,
/// that context has no scope instance, and the opened instance joins it.
pub struct PreparedConnection {
    root: Arc<ScopeContainer>,
    connections: ScopeOpener<WebsocketConnection>,
    messages: ScopeOpener<WebsocketMessage>,
    entries: Arc<MessageEntries>,
    upgrade: Arc<WebsocketUpgradeMeta>,
    meta: WsConnectionMeta,
    seeds: Seeds,
}

impl PreparedConnection {
    pub(super) fn new(
        root: Arc<ScopeContainer>,
        connections: ScopeOpener<WebsocketConnection>,
        messages: ScopeOpener<WebsocketMessage>,
        entries: Arc<MessageEntries>,
        upgrade: WebsocketUpgradeMeta,
        meta: WsConnectionMeta,
    ) -> Self {
        Self {
            root,
            connections,
            messages,
            entries,
            upgrade: Arc::new(upgrade),
            meta,
            seeds: Seeds::new(),
        }
    }

    /// The application root of the generation current at the upgrade. A handshake resolves
    /// singletons and configuration from it, since the connection scope is not open yet.
    pub fn root(&self) -> &Arc<ScopeContainer> {
        &self.root
    }

    /// The head of the HTTP request that initiated the upgrade. The opened scope holds it as
    /// its [`WebsocketUpgradeMeta`] seed.
    pub fn upgrade(&self) -> &WebsocketUpgradeMeta {
        &self.upgrade
    }

    /// A shared handle to the upgrade head, for a handshake that outlives this borrow. The
    /// opening moves the head into the scope when no handle is left, and copies it otherwise.
    pub fn upgrade_handle(&self) -> Arc<WebsocketUpgradeMeta> {
        Arc::clone(&self.upgrade)
    }

    /// What the upgrade negotiated, such as the selected subprotocol.
    pub fn meta(&self) -> &WsConnectionMeta {
        &self.meta
    }

    /// Fills the connection scope's factory-less slot for `T` with `value` when it opens.
    #[must_use]
    pub fn seed<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.seeds.insert(value);

        self
    }

    /// Fills the connection scope's factory-less slots with every seed of `seeds` when it
    /// opens.
    #[must_use]
    pub fn seeds(mut self, seeds: Seeds) -> Self {
        self.seeds.extend(seeds);

        self
    }

    /// Opens the connection scope with the upgrade metadata and the seeds added so far,
    /// building its seeds and eager components; every other connection-scoped component is
    /// built the first time something resolves it. The instance joins the invocation context
    /// `serve` runs in.
    ///
    /// # Errors
    ///
    /// As [`ScopeBuilder::open_on_demand`](upwell_app::ScopeBuilder::open_on_demand): a seed
    /// without a slot in the scope, two seeds for one slot, or a failing eager component. The
    /// error is logged; the protocol closes the socket, for example with
    /// [`scope_failure_close`].
    pub async fn open(self) -> upwell_app::Result<WsConnectionScope> {
        let Self {
            connections,
            messages,
            entries,
            upgrade,
            meta,
            seeds,
            ..
        } = self;
        let opening = connections
            .builder(WebsocketConnection)
            .seed(Arc::unwrap_or_clone(upgrade))
            .seed(meta)
            .seeds(seeds)
            .open_on_demand();
        let scope = opening.await.inspect_err(|error| {
            tracing::error!(
                target: "upwell::axum",
                %error,
                "ws connection scope build failed; closing socket"
            );
        })?;

        bind_opened_scope(scope.container());

        Ok(WsConnectionScope::new(scope, messages, entries))
    }
}

/// The close frame a protocol sends when [`PreparedConnection::open`] fails.
pub fn scope_failure_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: close_code::ERROR,
        reason: Utf8Bytes::from_static("connection scope build failed"),
    }))
}
