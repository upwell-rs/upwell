use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "tungstenite")]
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
#[cfg(feature = "tungstenite")]
use futures::StreamExt;
use upwell_app::AppRuntime;
use upwell_core::TypeDescriptor;
#[cfg(feature = "tungstenite")]
use upwell_test_utils::{TestEnvironment, TestServer, deadline};

use super::{
    WebsocketProtocol, WsAdmission, WsControllerDescriptor, WsControllerRegistration,
    WsDispatchError, WsFuture, WsHandlerFn, WsIdle, WsShutdown,
};
#[cfg(feature = "tungstenite")]
use super::{WebsocketUpgradeMeta, WsConnectionMeta};
use crate::AxumAppBuilder;

static TEST_PROTOCOL_BUILDS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn application_error_has_no_storage_for_sensitive_details() {
    let error = WsDispatchError::Application;

    assert_eq!(error.public_message(), "request failed");
    assert_eq!(error.to_string(), "ws application error");
}

struct TestProtocol;

impl WebsocketProtocol for TestProtocol {
    type Payload = ();
    type Outcome = ();
    type Options = ();
    type BuildError = std::convert::Infallible;

    fn build(
        _routes: &[WsControllerDescriptor],
        _runtime: &AppRuntime,
        _options: (),
    ) -> Result<Self, Self::BuildError> {
        TEST_PROTOCOL_BUILDS.fetch_add(1, Ordering::Relaxed);

        Ok(Self)
    }

    async fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: crate::ws::WsConnectionScope,
        shutdown: WsShutdown,
    ) {
        let _ = (self, socket, connection, shutdown);
    }
}

struct MultiEndpointProtocol;

static MULTI_ENDPOINT_BUILDS: AtomicUsize = AtomicUsize::new(0);
static MULTI_ENDPOINT_REGISTRATIONS: AtomicUsize = AtomicUsize::new(0);

impl WebsocketProtocol for MultiEndpointProtocol {
    type Payload = ();
    type Outcome = ();
    type Options = ();
    type BuildError = std::convert::Infallible;

    fn build(
        _controllers: &[WsControllerDescriptor],
        _runtime: &AppRuntime,
        _options: (),
    ) -> Result<Self, Self::BuildError> {
        MULTI_ENDPOINT_BUILDS.fetch_add(1, Ordering::Relaxed);

        Ok(Self)
    }

    async fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: crate::ws::WsConnectionScope,
        shutdown: WsShutdown,
    ) {
        let _ = (self, socket, connection, shutdown);
    }

    fn register(_registry: &mut upwell_app::AppRegistry) {
        MULTI_ENDPOINT_REGISTRATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn same_protocol_can_mount_at_distinct_paths_with_one_di_registration() {
    let app = crate::App::builder("dual-keyed-ws-test")
        .register_ws::<MultiEndpointProtocol>("/ws/one")
        .register_ws::<MultiEndpointProtocol>("/ws/two")
        .build()
        .await
        .expect("same protocol mounts on distinct paths");
    let paths: Vec<&str> = app
        .protocol()
        .ws_endpoints()
        .iter()
        .map(super::WebsocketHandler::path)
        .collect();

    assert_eq!(MULTI_ENDPOINT_BUILDS.load(Ordering::Relaxed), 2);
    assert_eq!(MULTI_ENDPOINT_REGISTRATIONS.load(Ordering::Relaxed), 1);
    assert_eq!(paths, ["/ws/one", "/ws/two"]);
}

#[test]
fn duplicate_mount_path_returns_typed_prepare_error() {
    let result = crate::App::builder("duplicate-ws-path-test")
        .register_ws::<TestProtocol>("/ws")
        .register_ws::<DuplicateProtocol>("/ws")
        .prepare();

    let error = match result {
        Ok(_) => panic!("duplicate WebSocket path was not rejected during preparation"),
        Err(error) => error,
    };

    assert!(matches!(error, crate::Error::Config(_)), "got: {error}");
}

#[tokio::test]
async fn old_signature_custom_protocol_mounts_without_adapter_methods() {
    let builds_before = TEST_PROTOCOL_BUILDS.load(Ordering::Relaxed);
    let app = crate::App::builder("old-signature-ws-test")
        .config_source(
            upwell_config::ConfigManager::<upwell_config::Toml>::empty()
                .with_resolvers(upwell_config::ResolverChain::empty()),
        )
        .register_ws::<TestProtocol>("/ws")
        .build()
        .await
        .expect("old-signature protocol mounts");

    assert_eq!(
        TEST_PROTOCOL_BUILDS.load(Ordering::Relaxed),
        builds_before + 1
    );
    assert_eq!(app.protocol().ws_endpoints().len(), 1);
    assert_eq!(app.protocol().ws_endpoints()[0].path(), "/ws");
}

#[test]
fn websocket_limits_fail_during_prepare_before_protocol_build() {
    let config = upwell_config::ConfigManager::<upwell_config::Toml>::from_str(
        r#"
            [axum]
            max_websocket_message_bytes = 0
        "#,
    )
    .expect("config parses");
    let builds_before = TEST_PROTOCOL_BUILDS.load(Ordering::Relaxed);
    let result = crate::App::builder("invalid-ws-config-test")
        .config_source(config)
        .register_ws::<TestProtocol>("/ws")
        .prepare();

    let error = match result {
        Ok(_) => panic!("zero WebSocket message limit was not rejected during preparation"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("must both be greater than zero"));
    assert_eq!(TEST_PROTOCOL_BUILDS.load(Ordering::Relaxed), builds_before);
}

struct DuplicateProtocol;

static DUPLICATE_PROTOCOL_BUILDS: AtomicUsize = AtomicUsize::new(0);

impl WebsocketProtocol for DuplicateProtocol {
    type Payload = ();
    type Outcome = ();
    type Options = ();
    type BuildError = std::convert::Infallible;

    fn build(
        _: &[WsControllerDescriptor],
        _: &AppRuntime,
        _: (),
    ) -> Result<Self, Self::BuildError> {
        DUPLICATE_PROTOCOL_BUILDS.fetch_add(1, Ordering::Relaxed);

        Ok(Self)
    }

    async fn serve(
        self: Arc<Self>,
        socket: WebSocket,
        connection: crate::ws::WsConnectionScope,
        shutdown: WsShutdown,
    ) {
        let _ = (self, socket, connection, shutdown);
    }
}

fn duplicate_handler() -> WsHandlerFn<DuplicateProtocol> {
    Arc::new(|(), _scope| -> WsFuture<DuplicateProtocol> { Box::pin(async { Ok(()) }) })
}

fn duplicate_route_descriptors() -> Vec<super::WsRouteDescriptor> {
    vec![super::WsRouteDescriptor::new::<DuplicateProtocol>(
        "messages.send",
        |_runtime| duplicate_handler(),
    )]
}

fn duplicate_protocol_id() -> std::any::TypeId {
    std::any::TypeId::of::<DuplicateProtocol>()
}

fn duplicate_protocol_name() -> &'static str {
    std::any::type_name::<DuplicateProtocol>()
}

fn duplicate_controller(id: &'static str) -> WsControllerDescriptor {
    WsControllerDescriptor::prepare(&WsControllerRegistration {
        id,
        name: "DuplicateController",
        ty: TypeDescriptor::of::<DuplicateProtocol>("DuplicateProtocol"),
        protocol: duplicate_protocol_id,
        protocol_name: duplicate_protocol_name,
        routes: duplicate_route_descriptors,
    })
}

#[test]
fn duplicate_destinations_fail_before_custom_protocol_build() {
    let builds_before = DUPLICATE_PROTOCOL_BUILDS.load(Ordering::Relaxed);
    let result = super::validate_unique_destinations(
        &[
            duplicate_controller("first"),
            duplicate_controller("second"),
        ],
        std::any::type_name::<DuplicateProtocol>(),
    );
    let error = result.expect_err("duplicate route must fail");

    assert!(error.to_string().contains("messages.send"));
    assert_eq!(
        DUPLICATE_PROTOCOL_BUILDS.load(Ordering::Relaxed),
        builds_before,
        "route validation must run before the downstream build implementation"
    );
}

#[test]
fn invalid_mount_path_fails_during_prepare() {
    let result = crate::App::builder("invalid-ws-path-test")
        .register_ws::<DuplicateProtocol>("ws")
        .prepare();

    let error = match result {
        Ok(_) => panic!("relative WebSocket path was not rejected during preparation"),
        Err(error) => error,
    };

    assert!(matches!(error, crate::Error::Config(_)), "got: {error}");
    assert_eq!(DUPLICATE_PROTOCOL_BUILDS.load(Ordering::Relaxed), 0);
}

#[test]
fn idle_timeout_probes_once_then_closes_unresponsive_peer() {
    let mut idle = WsIdle::new(Some(std::time::Duration::from_secs(10)));

    assert!(!idle.on_timeout(), "first idle interval sends a ping");
    assert!(idle.on_timeout(), "second idle interval closes the peer");

    idle.on_activity();

    assert!(
        !idle.on_timeout(),
        "peer activity clears the outstanding probe"
    );
}

#[test]
fn admission_permit_is_released_when_connection_finishes() {
    let admission = WsAdmission::new(1).expect("valid admission limit");
    let first = admission
        .try_acquire()
        .expect("admission is open")
        .expect("limit is enabled");

    assert!(admission.try_acquire().is_err(), "second peer is rejected");

    drop(first);

    assert!(
        admission
            .try_acquire()
            .expect("permit was released")
            .is_some(),
        "a later peer is admitted"
    );
}

#[test]
fn admission_accepts_tokio_boundary_and_rejects_oversized_config() {
    WsAdmission::new(tokio::sync::Semaphore::MAX_PERMITS)
        .expect("Tokio's maximum permit count is valid");

    let oversized = tokio::sync::Semaphore::MAX_PERMITS
        .checked_add(1)
        .expect("Tokio leaves room above MAX_PERMITS");
    let Err(error) = WsAdmission::new(oversized) else {
        panic!("oversized limit must fail app build");
    };

    assert!(error.to_string().contains("max_websocket_connections"));
    assert!(error.to_string().contains(&oversized.to_string()));
}

#[cfg(feature = "tungstenite")]
struct RequiredSubprotocol;

#[cfg(feature = "tungstenite")]
impl WebsocketProtocol for RequiredSubprotocol {
    type Payload = ();
    type Outcome = ();
    type Options = ();
    type BuildError = std::convert::Infallible;

    const SUBPROTOCOLS: &'static [&'static str] = &["test.v1"];
    const REQUIRE_SUBPROTOCOL: bool = true;

    fn build(
        _: &[WsControllerDescriptor],
        _: &AppRuntime,
        _: (),
    ) -> Result<Self, Self::BuildError> {
        Ok(Self)
    }

    async fn serve(
        self: Arc<Self>,
        mut socket: WebSocket,
        connection: crate::ws::WsConnectionScope,
        shutdown: WsShutdown,
    ) {
        let selected = connection
            .container()
            .extract::<WsConnectionMeta>()
            .await
            .expect("connection metadata resolves")
            .selected_subprotocol()
            .unwrap_or_default()
            .to_owned();
        let upgrade = connection
            .container()
            .extract::<WebsocketUpgradeMeta>()
            .await
            .expect("upgrade metadata resolves");

        let _ = socket
            .send(Message::Text(format!("{selected}|{}", upgrade.uri).into()))
            .await;
        let _ = (self, shutdown);
    }
}

#[cfg(feature = "tungstenite")]
#[tokio::test]
async fn required_subprotocol_is_negotiated_and_seeded() {
    let environment = TestEnvironment::new("upwell-axum-ws-");
    let app = crate::App::builder("ws-subprotocol-test")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<RequiredSubprotocol>("/ws")
        .build()
        .await
        .expect("subprotocol app builds");
    let server = TestServer::start_with_guard(app, environment).await;
    let address = server.address();
    let url = format!("ws://{address}/ws");

    let mut socket = deadline(
        "accepted subprotocol connect",
        tokio_tungstenite_wasm::connect_with_protocols(&url, &["other", "test.v1"]),
    )
    .await
    .expect("accepted subprotocol connects");
    let message = deadline("selected subprotocol receive", socket.next())
        .await
        .expect("selected protocol message")
        .expect("valid selected protocol message");

    assert_eq!(message.into_text().expect("text frame"), "test.v1|/ws");
    assert!(
        deadline(
            "missing subprotocol rejection",
            tokio_tungstenite_wasm::connect(&url),
        )
        .await
        .is_err(),
        "required protocol rejects a client that offers none"
    );

    server.shutdown().await;
}
