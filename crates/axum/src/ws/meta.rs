//! The metadata an upgrade captures and seeds into its connection scope.

use upwell_core::{Scope, TypeDescriptor};

/// Native HTTP metadata captured from the request that initiated a WebSocket upgrade.
#[derive(Clone, Debug)]
pub struct WebsocketUpgradeMeta {
    /// The upgrade request's HTTP method.
    pub method: axum::http::Method,

    /// The upgrade request's URI.
    pub uri: axum::http::Uri,

    /// The upgrade request's headers.
    pub headers: axum::http::HeaderMap,

    /// Cookies parsed from the upgrade request's `Cookie` headers.
    pub cookies: std::collections::HashMap<String, String>,
}

impl WebsocketUpgradeMeta {
    /// Captures metadata from a WebSocket upgrade request.
    pub fn from_parts(
        method: axum::http::Method,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
    ) -> Self {
        let request = crate::RequestMeta::from_parts(method, uri, headers);

        Self {
            method: request.method,
            uri: request.uri,
            headers: request.headers,
            cookies: request.cookies,
        }
    }
}

impl upwell_di::Injectable for WebsocketUpgradeMeta {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[cfg(feature = "di-check")]
impl upwell_di::Provided<WebsocketUpgradeMeta> for upwell_di::Wiring {}

pub(crate) static WEBSOCKET_UPGRADE_META_DESCRIPTOR: upwell_di::ComponentDescriptor =
    upwell_di::ComponentDescriptor::manual(
        "__upwell_websocket_upgrade_meta",
        "WebsocketUpgradeMeta",
        TypeDescriptor::of::<WebsocketUpgradeMeta>("WebsocketUpgradeMeta"),
        crate::scope::WebsocketConnection::KEY,
    );

/// Metadata selected while accepting one WebSocket upgrade.
#[derive(Clone, Debug)]
pub struct WsConnectionMeta {
    pub(super) selected_subprotocol: Option<String>,
}

impl WsConnectionMeta {
    /// The negotiated RFC 6455 subprotocol, if one was selected.
    pub fn selected_subprotocol(&self) -> Option<&str> {
        self.selected_subprotocol.as_deref()
    }
}

impl upwell_di::Injectable for WsConnectionMeta {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[cfg(feature = "di-check")]
impl upwell_di::Provided<WsConnectionMeta> for upwell_di::Wiring {}

pub(crate) static WS_CONNECTION_META_DESCRIPTOR: upwell_di::ComponentDescriptor =
    upwell_di::ComponentDescriptor::manual(
        "__upwell_ws_connection_meta",
        "WsConnectionMeta",
        TypeDescriptor::of::<WsConnectionMeta>("WsConnectionMeta"),
        crate::scope::WebsocketConnection::KEY,
    );
