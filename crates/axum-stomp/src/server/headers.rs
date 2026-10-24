//! Per-message DI seeds: the frame's [`StompHeaders`] and the connection's [`StompSession`].
//!
//! Both are **by-value injectables** (`Injectable<Target = Self>`, like `PeerInfo`): the serve loop
//! seeds them into each message's [`WebsocketMessage`](upwell_axum::WebsocketMessage) scope, and a `#[message]`
//! handler reaches them with `Inject<StompHeaders>` / `Inject<StompSession>` — the same DI a REST
//! route gets. Their manual [`ComponentDescriptor`]s are registered by the plugin so the container
//! knows each type's exact seed destination.

use std::sync::Arc;

use upwell_axum::Injectable;

use super::broker::{Broker, BrokerExt};
use crate::StompBody;
use upwell_axum::ConnectionId;

/// The headers of the STOMP frame that triggered the current message, in wire order (first value
/// wins per the spec). A cheap, `Arc`-backed clone so seeding it per message is nearly free.
#[derive(Clone, Debug, Default)]
pub struct StompHeaders {
    headers: Arc<Vec<(String, String)>>,
}

impl StompHeaders {
    /// Builds headers from an ordered `(name, value)` list.
    pub fn new(headers: Vec<(String, String)>) -> Self {
        Self {
            headers: Arc::new(headers),
        }
    }

    /// The first value for `name`, or `None`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The `destination` header, if present.
    pub fn destination(&self) -> Option<&str> {
        self.get("destination")
    }

    /// The `content-type` header, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.get("content-type")
    }

    /// Every header, in wire order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl Injectable for StompHeaders {
    type Target = StompHeaders;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// The current connection's handle onto the broker: lets a handler publish to any destination
/// imperatively (`session.publish(dest, body)`), independent of its own return value. The typed
/// [`Publisher`](super::Publisher) wraps this. Cheap to clone (an `Arc` + a `Copy` id).
#[derive(Clone)]
pub struct StompSession {
    broker: Arc<Broker>,
    connection: ConnectionId,
}

impl StompSession {
    /// Builds a session handle over `broker` for connection `connection`.
    pub fn new(broker: Arc<Broker>, connection: ConnectionId) -> Self {
        Self { broker, connection }
    }

    /// This message's connection id.
    pub fn connection(&self) -> ConnectionId {
        self.connection
    }

    /// The broker this session publishes through.
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Fans `body` out to every subscriber of `destination` (a raw publish; the typed
    /// [`Publisher`](super::Publisher) is the ergonomic front for this).
    pub fn publish(&self, destination: &str, body: &StompBody) {
        self.broker.publish(destination, body, &[]);
    }
}

impl Injectable for StompSession {
    type Target = StompSession;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// Under `di-check`, both are framework-seeded into every message scope (as `BoxedComponent`
/// seeds, no registered factory), so the compile-time checker treats them as always provided.
#[cfg(feature = "di-check")]
mod di_check {
    use super::{StompHeaders, StompSession};

    impl upwell_axum::Provided<StompHeaders> for upwell_axum::Wiring {}
    impl upwell_axum::Provided<StompSession> for upwell_axum::Wiring {}
}
