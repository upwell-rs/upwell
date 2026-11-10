//! The invocation contexts a WebSocket connection and its messages run in.

use upwell_core::Scope;
use upwell_di::context::InvocationContext;

use super::*;
use crate::scope::{WebsocketConnection, WebsocketMessage};
use crate::target::HttpTarget;

/// A protocol reporting the context its connection runs in, and the one a message it opens
/// would dispatch in.
struct ContextReport;

/// `<kind>:<scope>` for the active context, or `none` outside every context.
fn describe() -> String {
    let Ok(context) = InvocationContext::capture() else {
        return "none".to_owned();
    };

    let kind = match context.target().downcast_ref::<HttpTarget>() {
        Some(HttpTarget::WebsocketConnection) => "connection",
        Some(HttpTarget::WebsocketMessage) => "message",
        _ => "other",
    };

    let scope = match context.scope_key().map(|key| key.id()) {
        Some(id) if id == WebsocketConnection::ID => "connection",
        Some(id) if id == WebsocketMessage::ID => "message",
        _ => "other",
    };

    format!("{kind}:{scope}")
}

impl WebsocketProtocol for ContextReport {
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
        connection: crate::ws::PreparedConnection,
        shutdown: WsShutdown,
    ) {
        let before_open = describe();
        let connection = connection.open().await.expect("the connection scope opens");
        let in_connection = describe();
        let message = connection
            .open_message("undeclared", connection.message())
            .await
            .expect("a message scope opens");
        let in_message = connection
            .message_context(&message)
            .enter(async {
                tokio::task::yield_now().await;

                describe()
            })
            .await;
        let after = describe();

        let _ = socket
            .send(Message::Text(
                format!("{before_open}|{in_connection}|{in_message}|{after}").into(),
            ))
            .await;
        let _ = (self, shutdown);
    }
}

#[tokio::test]
async fn a_connection_joins_its_context_when_it_opens_and_each_message_runs_in_its_own() {
    let environment = TestEnvironment::new("upwell-axum-ws-context-");
    let app = crate::App::builder("ws-context-test")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<ContextReport>("/ws")
        .build()
        .await
        .expect("the app builds");
    let server = TestServer::start_with_guard(app, environment).await;
    let url = format!("ws://{}/ws", server.address());

    let mut socket = deadline("connect", tokio_tungstenite_wasm::connect(&url))
        .await
        .expect("the socket connects");
    let report = deadline("receive the report", socket.next())
        .await
        .expect("a report")
        .expect("a valid frame");

    assert_eq!(
        report.into_text().expect("a text frame"),
        "connection:other|connection:connection|message:message|connection:connection"
    );

    server.shutdown().await;
}
