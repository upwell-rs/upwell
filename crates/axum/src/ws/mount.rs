//! Mounting one WebSocket endpoint and its generic upgrade handler.

use std::sync::Arc;

use axum::extract::FromRequestParts as _;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse as _;
use upwell_app::{AppRuntime, ScopeOpener};
use upwell_config::ContainerConfigExt;
use upwell_core::Scope;
use upwell_di::context::TargetRef;

use super::connection::{self, MessageEntries};
use super::settings::WsAdmission;
use super::{
    PreparedConnection, WebsocketProtocol, WebsocketUpgradeMeta, WsConnectionMeta,
    WsControllerDescriptor, WsShutdown, validate_config,
};
use crate::scope::{WebsocketConnection, WebsocketMessage};
use crate::target::HttpTarget;

/// The app-side management handle for one mounted ws endpoint: its path and protocol, and the
/// trigger that drains its live connections on graceful shutdown. Held by the [`Axum`](crate::Axum)
/// protocol so endpoints can be inspected and shut down.
pub struct WebsocketHandler {
    path: String,
    protocol_name: &'static str,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl WebsocketHandler {
    /// The upgrade path this endpoint is mounted at.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The name of the protocol serving this endpoint.
    pub fn protocol_name(&self) -> &'static str {
        self.protocol_name
    }

    /// Signals every live connection on this endpoint to drain and close.
    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

/// Mounts one ws endpoint: builds the protocol `P` from its controllers and `options`, wires the
/// framework's generic upgrade handler at `path`, and returns a path-scoped router plus its
/// management handle. Monomorphized per `P`; `register_ws` stores a closure that calls it.
pub(crate) fn mount_ws<P: WebsocketProtocol>(
    path: &str,
    controllers: Vec<WsControllerDescriptor>,
    runtime: &AppRuntime,
    options: P::Options,
) -> crate::Result<(axum::Router, WebsocketHandler)> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let config = runtime
        .root()
        .config::<crate::AxumConfig>(crate::AXUM_CONFIG_PATH)
        .expect("AxumConfig missing from config store; Axum should register it")
        .snapshot();

    validate_config(&config)?;

    let protocol = Arc::new(P::build(&controllers, runtime, options).map_err(|source| {
        crate::Error::WebsocketBuild {
            protocol: std::any::type_name::<P>(),
            source: Box::new(source),
        }
    })?);
    let messages = runtime.opener::<WebsocketMessage>()?;
    let endpoint = Arc::new(Endpoint {
        protocol,
        shutdown: WsShutdown(rx),
        admission: WsAdmission::new(config.max_websocket_connections)?,
        max_message_bytes: config.max_websocket_message_bytes,
        max_frame_bytes: config.max_websocket_frame_bytes,
        runtime: runtime.clone(),
        connections: runtime.opener::<WebsocketConnection>()?,
        entries: Arc::new(connection::message_entries(&controllers, &messages)?),
        messages,
    });
    let route_handler = move |request: axum::extract::Request| {
        let endpoint = Arc::clone(&endpoint);

        async move { endpoint.upgrade(request).await }
    };
    let router = axum::Router::new().route(path, axum::routing::any(route_handler));
    let handler = WebsocketHandler {
        path: path.to_string(),
        protocol_name: std::any::type_name::<P>(),
        shutdown: tx,
    };

    Ok((router, handler))
}

/// Everything the upgrade handler of one endpoint shares across its connections.
struct Endpoint<P: WebsocketProtocol> {
    protocol: Arc<P>,
    shutdown: WsShutdown,
    admission: WsAdmission,
    max_message_bytes: usize,
    max_frame_bytes: usize,
    runtime: AppRuntime,
    connections: ScopeOpener<WebsocketConnection>,
    messages: ScopeOpener<WebsocketMessage>,
    entries: Arc<MessageEntries>,
}

impl<P: WebsocketProtocol> Endpoint<P> {
    /// Accepts one upgrade request, then hands the socket and its unopened connection scope to
    /// the protocol, which owns the read, decode, dispatch, encode and send loop and decides
    /// when the connection scope opens.
    async fn upgrade(&self, request: axum::extract::Request) -> axum::response::Response {
        let (mut head, _body) = request.into_parts();
        let ws = match WebSocketUpgrade::from_request_parts(&mut head, &()).await {
            Ok(ws) => ws,
            Err(rejection) => return rejection.into_response(),
        };

        let permit = match self.admission.try_acquire() {
            Ok(permit) => permit,

            Err(_) => {
                tracing::warn!(
                    target: "upwell::axum",
                    "websocket connection limit reached; rejecting upgrade"
                );

                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        };

        let ws = ws
            .max_message_size(self.max_message_bytes)
            .max_frame_size(self.max_frame_bytes)
            .protocols(P::SUBPROTOCOLS.iter().copied());
        let selected_subprotocol = ws
            .selected_protocol()
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        if P::REQUIRE_SUBPROTOCOL && selected_subprotocol.is_none() {
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }

        let protocol = Arc::clone(&self.protocol);
        let shutdown = self.shutdown.clone();
        let context = self
            .connections
            .context(TargetRef::from_static(&HttpTarget::WebsocketConnection))
            .with_pending_scope(WebsocketConnection::KEY, None);
        let connection = PreparedConnection::new(
            self.runtime.root(),
            self.connections.clone(),
            self.messages.clone(),
            Arc::clone(&self.entries),
            WebsocketUpgradeMeta::from_parts(head.method, head.uri, head.headers),
            WsConnectionMeta {
                selected_subprotocol,
            },
        );

        ws.on_upgrade(move |socket| async move {
            let _permit = permit;

            context
                .enter(protocol.serve(socket, connection, shutdown))
                .await;
        })
        .into_response()
    }
}
