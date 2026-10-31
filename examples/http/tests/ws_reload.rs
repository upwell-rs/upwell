//! End-to-end tests for WebSocket messages across a runtime commit: a real ws client sends
//! JSON-WS frames to a served app whose config is rewritten and reloaded mid-connection.
//!
//! [`Stamp`] is a singleton built from the config value of its runtime generation, so the
//! value a scoped component reads through it names the generation that component resolved on.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use upwell::axum::prelude::*;
use upwell::config::{ResolverChain, Toml};
use upwell::prelude::*;
use upwell::{AppRuntime, ConfigManager, component, config, methods};
use upwell_test_utils::{TempFixture, TestServer, deadline};

app! {
    /// Generated host for WebSocket reload tests.
    app WebsocketReloadApplication {
        name: "ws-reload-test",
        protocol: upwell::axum::Axum,
    }
}

/// Fired when a parked message has resolved its message scope.
static PARKED: Notify = Notify::const_new();

/// Lets a parked message continue.
static RELEASE: Notify = Notify::const_new();

#[config(path = "stamp")]
#[derive(serde::Deserialize)]
struct StampCfg {
    value: u32,
}

/// A singleton holding the config value its generation was built from.
#[component]
struct Stamp {
    #[default]
    value: u32,
}

#[methods]
impl Stamp {
    #[init]
    fn new(cfg: Cfg<StampCfg>) -> Self {
        Self {
            value: cfg.get().value,
        }
    }
}

/// A connection-scoped reader of [`Stamp`].
#[component(scope = WebsocketConnection)]
struct ConnStamp {
    stamp: Arc<Stamp>,
}

/// A message-scoped reader of [`Stamp`].
#[component(scope = WebsocketMessage)]
struct MessageStamp {
    stamp: Arc<Stamp>,
}

/// The stamps one message read through its message and connection scopes.
#[dto]
struct Stamps {
    message: u32,
    connection: u32,
}

#[controller(ws = JsonWs)]
struct StampSock;

#[handlers(ws = JsonWs)]
impl StampSock {
    /// Injects nothing, so a reply only proves the connection is open.
    #[message("ping")]
    async fn ping(&self) -> u32 {
        0
    }

    /// The stamps the message's message and connection scopes read.
    #[message("stamps")]
    async fn stamps(
        &self,
        Inject(message): Inject<Arc<MessageStamp>>,
        Inject(connection): Inject<Arc<ConnStamp>>,
    ) -> Stamps {
        Stamps {
            message: message.stamp.value,
            connection: connection.stamp.value,
        }
    }

    /// Resolves its message stamp, parks until released, then builds the connection stamp.
    #[message("parked")]
    async fn parked(
        &self,
        Inject(message): Inject<Arc<MessageStamp>>,
        Inject(connection): Inject<Lazy<Arc<ConnStamp>>>,
    ) -> Stamps {
        PARKED.notify_one();
        RELEASE.notified().await;

        let connection = connection
            .get_or_create()
            .await
            .expect("the connection stamp builds on demand");

        Stamps {
            message: message.stamp.value,
            connection: connection.stamp.value,
        }
    }
}

/// A served app over a config file holding `value`, with its runtime and the config fixture.
async fn start(value: u32) -> (TestServer<upwell::axum::Axum>, AppRuntime, TempFixture) {
    let fixture = TempFixture::new("upwell-http-ws-reload-");

    write_value(&fixture, value);

    let config =
        ConfigManager::<Toml>::load_in_with_resolvers(fixture.path(), &[], ResolverChain::empty())
            .expect("load config");
    let app = WebsocketReloadApplication::builder()
        .expect("app builder")
        .config_source(config)
        .directories(upwell::dirs::DirectoriesManager::from_path(
            fixture.child("dirs"),
        ))
        .register_ws::<JsonWs>("/ws")
        .build()
        .await
        .expect("app builds");
    let runtime = app.runtime().clone();

    (
        TestServer::start_with_guard(app, ()).await,
        runtime,
        fixture,
    )
}

fn write_value(fixture: &TempFixture, value: u32) {
    fixture.write("application.toml", format!("[stamp]\nvalue = {value}\n"));
}

/// Rewrites the config to `value` and commits the generation built from it.
async fn commit(runtime: &AppRuntime, fixture: &TempFixture, value: u32) {
    write_value(fixture, value);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(
        report.is_committed(),
        "a changed value commits a generation"
    );
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(server: &TestServer<upwell::axum::Axum>) -> Socket {
    let (socket, _) = deadline(
        "WebSocket connect",
        tokio_tungstenite::connect_async(format!("ws://{}/ws", server.address())),
    )
    .await
    .expect("ws connect");

    socket
}

/// Sends a frame to `dest` and returns the reply's `ok` value.
async fn send(socket: &mut Socket, dest: &str) -> serde_json::Value {
    let frame = format!(r#"{{"dest":"{dest}","id":1,"payload":null}}"#);

    deadline("send frame", socket.send(Message::Text(frame.into())))
        .await
        .expect("send frame");

    let reply = deadline("receive reply", async {
        loop {
            let message = socket.next().await.expect("a frame").expect("ok frame");

            if let Message::Text(text) = message {
                return serde_json::from_str::<serde_json::Value>(&text).expect("json reply");
            }
        }
    })
    .await;

    reply["ok"].clone()
}

fn stamps(message: u32, connection: u32) -> serde_json::Value {
    serde_json::json!({ "message": message, "connection": connection })
}

#[tokio::test]
async fn a_message_in_flight_across_a_commit_resolves_on_its_own_generation() {
    let (server, runtime, fixture) = start(1).await;
    let mut socket = connect(&server).await;

    let (reply, ()) = tokio::join!(send(&mut socket, "parked"), async {
        PARKED.notified().await;
        commit(&runtime, &fixture, 2).await;
        RELEASE.notify_one();
    });

    assert_eq!(
        reply,
        stamps(1, 1),
        "a slot built on demand after the commit still comes from the message's generation"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn messages_on_a_connection_opened_before_a_commit_stay_on_its_generation() {
    let (server, runtime, fixture) = start(1).await;
    let mut old = connect(&server).await;
    let opened = send(&mut old, "ping").await;

    commit(&runtime, &fixture, 2).await;

    let mut new = connect(&server).await;
    let on_old = send(&mut old, "stamps").await;
    let on_new = send(&mut new, "stamps").await;

    assert_eq!(opened, 0);
    assert_eq!(
        on_old,
        stamps(1, 1),
        "messages, and connection slots built on demand, run on their connection's generation"
    );
    assert_eq!(
        on_new,
        stamps(2, 2),
        "a new connection opens on the committed one"
    );

    server.shutdown().await;
}
