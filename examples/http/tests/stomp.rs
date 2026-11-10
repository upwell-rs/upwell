//! End-to-end test of a **STOMP pub/sub controller**: build the app, serve it on an ephemeral
//! port, then drive it with the *generated typed clients* — no destination strings at the call
//! sites. One client subscribes to a topic via `ChatTopicsClient::subscribe_room()`; another sends
//! to an app destination via `ChatControllerClient::chat(..)`, whose handler publishes to the
//! topic; the subscriber's typed `Subscription` stream then yields the broadcast message. The
//! server is shut down at the end so the test never hangs.

use std::sync::Arc;

use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use upwell::ScopeContainer;
use upwell::axum::client::ReqwestClient;
use upwell::axum::prelude::*;
use upwell::axum::{
    CodecError, Injected, StompAuthFuture, StompAuthenticationError, StompAuthenticator, StompBody,
    StompClientTransport, StompCodec, StompConfig, StompConnect, StompConnectOptions,
    StompPrincipal, TopicCodec,
};
use upwell::prelude::*;
use upwell_test_utils::{TestEnvironment, TestServer, deadline};

mod context_report;

app! {
    /// Generated host shared by the STOMP integration tests.
    app StompTestApplication {
        name: "stomp-test",
        protocol: upwell::axum::Axum,
    }
}

/// A message a client sends to the app (`/app/chat`).
#[dto]
struct SendChat {
    text: String,
}

/// A message broadcast to subscribers of `/topic/room`.
#[dto]
#[derive(Clone)]
struct RoomMsg {
    text: String,
}

/// The app's broadcast topics — the single source of truth for both sides. Generates
/// `impl Topic for ChatTopics` (server publish) and `ChatTopicsClient<C>::subscribe_room()`
/// (client subscribe), typed to `RoomMsg`.
#[topics(protocol = Stomp)]
enum ChatTopics {
    #[topic("/topic/room")]
    Room(RoomMsg),
}

/// A STOMP controller: inbound `SEND /app/chat` is handled and re-broadcast to `/topic/room`.
#[controller(ws = Stomp)]
struct Chat {}

#[handlers(ws = Stomp)]
impl Chat {
    /// Handles an inbound chat SEND and publishes it to the room topic (typed, via `Publisher`),
    /// tagged with the invocation context the handler ran in.
    #[message("/app/chat")]
    async fn chat(
        &self,
        msg: SendChat,
        Inject(publisher): Inject<Publisher<ChatTopics>>,
    ) -> Result<(), CodecError> {
        publisher
            .publish_to::<1>(ChatTopics::Room(RoomMsg {
                text: format!("{} ({})", msg.text, context_report::describe()),
            }))
            .await
    }
}

/// A normal HTTP controller can publish to the same STOMP topics as a STOMP controller.
#[controller(path = "/events")]
struct RestEvents {}

#[handlers]
impl RestEvents {
    #[post("/room")]
    async fn room(
        &self,
        Inject(publisher): Inject<Publisher<ChatTopics>>,
        Json(msg): Json<RoomMsg>,
    ) -> Json<RoomMsg> {
        // A REST endpoint emits fire-and-forget: no `await`, just the encode result.
        publisher
            .emit(ChatTopics::Room(msg.clone()))
            .expect("test topic encodes");

        Json(msg)
    }
}

#[tokio::test]
async fn stomp_send_is_broadcast_to_typed_subscribers() {
    let environment = TestEnvironment::new("upwell-http-stomp-");
    let app = StompTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<Stomp>("/stomp")
        .build()
        .await
        .expect("app builds");

    let server = TestServer::start_with_guard(app, environment).await;
    let addr = server.address();

    let url = format!("ws://{addr}/stomp");

    // ONE connection, shared across both typed client facades. `StompClientTransport` *is* the
    // connection and is cheaply `Clone` (a handle onto one actor + socket); cloning it does not
    // dial again. Send and subscribe here therefore ride the same socket.
    let connection = deadline("STOMP connect", StompClientTransport::connect(&url))
        .await
        .expect("connects");

    let mut room = deadline(
        "subscribe to room",
        ChatTopicsClient::new(connection.clone()).subscribe_room(),
    )
    .await
    .expect("subscribe_room");

    // Send to /app/chat via the generated typed method (no destination string), over the same
    // connection; the handler re-broadcasts to /topic/room, which this same socket is subscribed to.
    deadline(
        "send chat message",
        ChatClient::new(connection.clone()).chat(SendChat {
            text: "hello stomp".into(),
        }),
    )
    .await
    .expect("chat send");

    // The handler re-broadcast to /topic/room; the typed subscription yields the decoded message.
    let received = deadline("receive room broadcast", room.next())
        .await
        .expect("the subscription stream is live")
        .expect("a decoded RoomMsg");

    assert_eq!(received.text, "hello stomp (message:message)");

    server.shutdown().await;
}

#[tokio::test]
async fn stomp_connect_authentication_and_explicit_disconnect_share_lifecycle() {
    let environment = TestEnvironment::new("upwell-http-stomp-auth-");
    let config = StompConfig::default().with_authenticator(|connect: StompConnect| async move {
        if connect.login() == Some("alice")
            && connect.passcode() == Some("secret")
            && connect.headers().get("tenant") == Some("acme")
        {
            Ok(StompPrincipal::authenticated("alice").with_attribute("tenant", "acme"))
        } else {
            Err(StompAuthenticationError::new("invalid credentials"))
        }
    });
    let app = StompTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws_with::<Stomp>("/stomp", config)
        .build()
        .await
        .expect("app builds");

    let server = TestServer::start_with_guard(app, environment).await;
    let addr = server.address();
    let url = format!("ws://{addr}/stomp");

    let rejected = deadline(
        "rejected STOMP connect",
        StompClientTransport::connect_with_options(
            &url,
            StompConnectOptions::new()
                .with_login("mallory")
                .with_passcode("wrong"),
        ),
    )
    .await;
    assert!(rejected.is_err(), "invalid credentials must be rejected");

    let connection = deadline(
        "authenticated STOMP connect",
        StompClientTransport::connect_with_options(
            &url,
            StompConnectOptions::new()
                .with_login("alice")
                .with_passcode("secret")
                .with_header("tenant", "acme"),
        ),
    )
    .await
    .expect("valid credentials connect");
    let generated_client_handle = connection.clone();

    assert!(connection.is_connected());
    deadline("authenticated STOMP disconnect", connection.disconnect())
        .await
        .expect("disconnect cleanly");
    assert!(!connection.is_connected());
    assert!(
        !generated_client_handle.is_connected(),
        "all cloned/generated client handles observe the shared disconnect"
    );
    deadline("idempotent STOMP disconnect", connection.disconnect())
        .await
        .expect("disconnect is idempotent");

    server.shutdown().await;
}

/// A stand-in user directory for the DI-native authenticator tests. A singleton component, so an
/// authenticator can inject `Arc<UserStore>` as a parameter or hold it as a field.
#[component]
struct UserStore;

impl UserStore {
    /// Accepts the demo credentials (`alice` / `secret`).
    fn validate(&self, login: Option<&str>, passcode: Option<&str>) -> bool {
        login == Some("alice") && passcode == Some("secret")
    }
}

/// A component authenticator: DI injects its [`UserStore`] dependency, then `authenticate` validates
/// the CONNECT credentials against it. Installed with `Injected::<TokenAuth>::new()`.
#[component]
struct TokenAuth {
    users: Arc<UserStore>,
}

#[methods]
impl TokenAuth {
    #[init]
    async fn init(users: Arc<UserStore>) -> Self {
        Self { users }
    }
}

impl StompAuthenticator for TokenAuth {
    fn authenticate(
        self: Arc<Self>,
        connect: StompConnect,
        _root: Arc<ScopeContainer>,
    ) -> StompAuthFuture {
        Box::pin(async move {
            if self.users.validate(connect.login(), connect.passcode()) {
                Ok(StompPrincipal::authenticated("alice").into())
            } else {
                Err(StompAuthenticationError::new("invalid credentials"))
            }
        })
    }
}

/// Serves a STOMP endpoint configured with `config` and asserts that the demo credentials
/// (`alice` / `secret`) are accepted while others are rejected — the shared body of the two
/// DI-native authenticator tests.
async fn assert_credentials_are_enforced(config: StompConfig) {
    let environment = TestEnvironment::new("upwell-http-stomp-credentials-");
    let app = StompTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws_with::<Stomp>("/stomp", config)
        .build()
        .await
        .expect("app builds");

    let server = TestServer::start_with_guard(app, environment).await;
    let addr = server.address();
    let url = format!("ws://{addr}/stomp");

    let rejected = deadline(
        "rejected injected-auth STOMP connect",
        StompClientTransport::connect_with_options(
            &url,
            StompConnectOptions::new()
                .with_login("mallory")
                .with_passcode("wrong"),
        ),
    )
    .await;
    assert!(rejected.is_err(), "invalid credentials must be rejected");

    let connection = deadline(
        "authenticated injected-auth STOMP connect",
        StompClientTransport::connect_with_options(
            &url,
            StompConnectOptions::new()
                .with_login("alice")
                .with_passcode("secret"),
        ),
    )
    .await
    .expect("valid credentials connect");
    assert!(connection.is_connected());

    deadline("injected-auth STOMP disconnect", connection.disconnect())
        .await
        .expect("disconnect cleanly");

    server.shutdown().await;
}

#[tokio::test]
async fn stomp_authenticator_injects_dependencies_from_params() {
    let config = StompConfig::default().with_authenticator(
        |connect: StompConnect, users: Arc<UserStore>| async move {
            if users.validate(connect.login(), connect.passcode()) {
                Ok(StompPrincipal::authenticated("alice"))
            } else {
                Err(StompAuthenticationError::new("invalid credentials"))
            }
        },
    );

    assert_credentials_are_enforced(config).await;
}

#[tokio::test]
async fn stomp_authenticator_resolves_a_di_component() {
    let config = StompConfig::default().with_authenticator(Injected::<TokenAuth>::new());

    assert_credentials_are_enforced(config).await;
}

#[tokio::test]
async fn http_handler_can_publish_to_typed_stomp_subscribers() {
    let environment = TestEnvironment::new("upwell-http-stomp-rest-");
    let app = StompTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<Stomp>("/stomp")
        .build()
        .await
        .expect("app builds");

    let server = TestServer::start_with_guard(app, environment).await;
    let addr = server.address();

    let ws_url = format!("ws://{addr}/stomp");
    let connection = deadline(
        "HTTP publish STOMP connect",
        StompClientTransport::connect(&ws_url),
    )
    .await
    .expect("connects");

    let mut room = deadline(
        "subscribe for HTTP publish",
        ChatTopicsClient::new(connection).subscribe_room(),
    )
    .await
    .expect("subscribe_room");

    let http = RestEventsClient::new(ReqwestClient::new(format!("http://{addr}")));
    let response = deadline(
        "HTTP publish request",
        http.room(RoomMsg {
            text: "via rest".into(),
        }),
    )
    .await
    .expect("http publish");

    assert_eq!(response.text, "via rest");

    let received = deadline("receive HTTP-triggered broadcast", room.next())
        .await
        .expect("the subscription stream is live")
        .expect("a decoded RoomMsg");

    assert_eq!(received.text, "via rest");

    server.shutdown().await;
}

/// A deliberately non-JSON codec: it prepends a marker byte to the JSON and strips it on decode. If
/// either end of the SEND/broadcast path silently used plain JSON instead of this codec, the marker
/// byte would be missing (or unexpected) and decoding would fail — so a passing round trip proves
/// the codec is honored on **both** sides.
struct MarkedCodec;

const MARKER: u8 = 0xFE;

impl StompCodec for MarkedCodec {
    fn encode<T: Serialize>(value: &T) -> Result<StompBody, CodecError> {
        let mut bytes = vec![MARKER];
        bytes.extend(serde_json::to_vec(value).map_err(|e| CodecError::internal(e.to_string()))?);

        Ok(StompBody {
            content_type: Some("application/x-marked".to_owned()),
            bytes: bytes.into(),
        })
    }

    fn decode<T: DeserializeOwned>(body: StompBody) -> Result<T, CodecError> {
        match body.bytes.split_first() {
            Some((&MARKER, rest)) => {
                serde_json::from_slice(rest).map_err(|e| CodecError::bad_input(e.to_string()))
            }

            _ => Err(CodecError::bad_input(
                "body is missing the codec's marker byte",
            )),
        }
    }
}

impl TopicCodec<Stomp> for MarkedCodec {
    fn encode<T: Serialize>(value: &T) -> Result<StompBody, CodecError> {
        <Self as StompCodec>::encode(value)
    }

    fn decode<T: DeserializeOwned>(body: StompBody) -> Result<T, CodecError> {
        <Self as StompCodec>::decode(body)
    }
}

/// A topic set using the custom codec on both publish and subscribe.
#[topics(protocol = Stomp, codec = MarkedCodec)]
enum MarkedTopics {
    #[topic("/topic/marked")]
    Marked(RoomMsg),
}

/// A controller whose SEND payloads use the custom codec (via `codec = MarkedCodec`).
#[controller(ws = Stomp)]
struct Marked {}

#[handlers(ws = Stomp, codec = MarkedCodec)]
impl Marked {
    #[message("/app/marked")]
    async fn marked(
        &self,
        msg: SendChat,
        Inject(publisher): Inject<Publisher<MarkedTopics>>,
    ) -> Result<(), CodecError> {
        publisher
            .publish_to::<1>(MarkedTopics::Marked(RoomMsg { text: msg.text }))
            .await
    }
}

#[tokio::test]
async fn a_custom_codec_is_honored_on_both_ends_of_the_send_path() {
    let environment = TestEnvironment::new("upwell-http-stomp-codec-");
    let app = StompTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<Stomp>("/stomp")
        .build()
        .await
        .expect("app builds");

    let server = TestServer::start_with_guard(app, environment).await;
    let addr = server.address();

    let url = format!("ws://{addr}/stomp");
    let connection = deadline(
        "custom-codec STOMP connect",
        StompClientTransport::connect(&url),
    )
    .await
    .expect("connects");

    let mut marked = deadline(
        "subscribe with custom codec",
        MarkedTopicsClient::new(connection.clone()).subscribe_marked(),
    )
    .await
    .expect("subscribe_marked");

    // The generated `marked` SEND encodes with MarkedCodec; the server decodes with MarkedCodec,
    // re-publishes with MarkedCodec, and the subscription decodes with MarkedCodec.
    deadline(
        "send with custom codec",
        MarkedClient::new(connection.clone()).marked(SendChat {
            text: "via marker".into(),
        }),
    )
    .await
    .expect("marked send");

    let received = deadline("receive custom-codec broadcast", marked.next())
        .await
        .expect("the subscription stream is live")
        .expect("a decoded RoomMsg — proving MarkedCodec round-tripped on every hop");

    assert_eq!(received.text, "via marker");

    server.shutdown().await;
}
