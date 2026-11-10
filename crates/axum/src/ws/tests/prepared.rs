//! A protocol opens its connection scope itself, with seeds its handshake produced.

use upwell_app::AppRegistry;
use upwell_core::Scope;
use upwell_di::{ComponentDescriptor, Injectable};

use super::*;
use crate::scope::WebsocketConnection;
use crate::ws::{PreparedConnection, scope_failure_close};

/// A seed a protocol produces before it opens the connection scope.
#[derive(Clone, Debug)]
struct Greeting(String);

impl Injectable for Greeting {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// The connection scope's seed slot for [`Greeting`].
static GREETING: ComponentDescriptor = ComponentDescriptor::manual(
    "greeting",
    "Greeting",
    TypeDescriptor::of::<Greeting>("Greeting"),
    WebsocketConnection::KEY,
);

/// A value with no slot in the connection scope.
struct Stray;

/// A protocol that greets with the upgrade's path, seeding the greeting before it opens the
/// connection scope; on `/stray` it seeds a value without a slot, so the opening fails.
struct Greeter;

impl WebsocketProtocol for Greeter {
    type Payload = ();
    type Outcome = ();
    type Options = ();
    type BuildError = std::convert::Infallible;

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
        connection: PreparedConnection,
        shutdown: WsShutdown,
    ) {
        let path = connection.upgrade().uri.path().to_owned();
        let connection = match path.as_str() {
            "/stray" => connection.seed(Stray),
            _ => connection.seed(Greeting(format!("hello {path}"))),
        };
        let Ok(connection) = connection.open().await else {
            let _ = socket.send(scope_failure_close()).await;

            return;
        };

        let greeting = connection
            .container()
            .extract::<Greeting>()
            .await
            .expect("the seeded greeting resolves");

        let _ = socket.send(Message::Text(greeting.0.into())).await;
        let _ = (self, shutdown);
    }

    fn register(registry: &mut AppRegistry) {
        registry.components.push(GREETING);
    }
}

#[tokio::test]
async fn a_protocol_seeds_the_connection_scope_it_opens() {
    let environment = TestEnvironment::new("upwell-axum-ws-prepared-");
    let app = crate::App::builder("ws-prepared-test")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<Greeter>("/greet")
        .register_ws::<Greeter>("/stray")
        .build()
        .await
        .expect("the app builds");
    let server = TestServer::start_with_guard(app, environment).await;
    let address = server.address();

    let mut greeted = deadline(
        "connect",
        tokio_tungstenite_wasm::connect(format!("ws://{address}/greet")),
    )
    .await
    .expect("the socket connects");
    let greeting = deadline("receive the greeting", greeted.next())
        .await
        .expect("a greeting")
        .expect("a valid frame");

    assert_eq!(greeting.into_text().expect("a text frame"), "hello /greet");

    let mut stray = deadline(
        "connect",
        tokio_tungstenite_wasm::connect(format!("ws://{address}/stray")),
    )
    .await
    .expect("the socket connects");
    let closed = deadline("receive the close", stray.next())
        .await
        .expect("a close frame")
        .expect("a valid frame");

    assert!(
        matches!(closed, tokio_tungstenite_wasm::Message::Close(Some(_))),
        "a failed opening closes the socket: {closed:?}"
    );

    server.shutdown().await;
}
