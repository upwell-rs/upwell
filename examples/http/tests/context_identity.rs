#![allow(dead_code)]

//! One app-defined identity over REST and STOMP (#127, #128): REST seeds it into each request
//! with a `RequestSeeder`, STOMP into each connection from its authenticator, and one
//! request-or-message-scoped component reads it on both paths.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use upwell::axum::axum::http::StatusCode;
use upwell::axum::axum::http::request::Parts;
use upwell::axum::axum::response::{IntoResponse, Response};
use upwell::axum::prelude::*;
use upwell::axum::{
    CodecError, RequestSeeder, StompAuthenticationError, StompClientTransport, StompConnectOptions,
};
use upwell::prelude::*;
use upwell_test_utils::{TestEnvironment, TestServer, deadline};

app! {
    /// Generated host shared by the identity tests.
    app IdentityApplication {
        name: "context-identity-test",
        protocol: upwell::axum::Axum,
    }
}

/// Who a request or a connection runs as: seeded per request over REST and per connection over
/// STOMP.
#[context(scopes = [WebsocketConnection, HttpRequest])]
#[derive(Clone)]
struct Identity {
    subject: String,
}

/// Greets the identity: one component type, per request over REST and per message over STOMP.
#[component(scope = [WebsocketMessage, HttpRequest])]
struct Greeting {
    identity: Identity,
}

impl Greeting {
    fn text(&self) -> String {
        format!("hello {}", self.identity.subject)
    }
}

/// Built on every controller request with the request's identity. A WebSocket upgrade opens no
/// request scope, so it never builds one.
#[component(scope = HttpRequest, eager)]
struct Audit {
    identity: Identity,
}

/// Seeds the identity from the `x-user` header, rejecting a request without one.
#[component]
struct HeaderIdentity;

impl RequestSeeder for HeaderIdentity {
    type Seed = Identity;

    async fn seed(&self, request: &Parts) -> Result<Identity, Response> {
        request
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|subject| Identity {
                subject: subject.to_owned(),
            })
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

/// A second seeder of the identity, which conflicts with [`HeaderIdentity`].
#[component]
struct AnonymousIdentity;

impl RequestSeeder for AnonymousIdentity {
    type Seed = Identity;

    async fn seed(&self, _request: &Parts) -> Result<Identity, Response> {
        Ok(Identity {
            subject: String::from("anonymous"),
        })
    }
}

/// The REST greeting.
#[controller(path = "/greet")]
struct RestGreeting {}

#[handlers]
impl RestGreeting {
    #[get("/")]
    async fn greet(&self, Inject(greeting): Inject<Arc<Greeting>>) -> String {
        greeting.text()
    }
}

/// A request a STOMP client sends for its greeting.
#[dto]
struct GreetAsk {}

/// The STOMP greeting.
#[dto]
struct GreetReply {
    text: String,
}

/// The STOMP greeting controller.
#[controller(ws = Stomp)]
struct StompGreeting {}

#[handlers(ws = Stomp)]
impl StompGreeting {
    #[message("/app/greet")]
    async fn greet(
        &self,
        _ask: GreetAsk,
        Inject(greeting): Inject<Arc<Greeting>>,
    ) -> Result<GreetReply, CodecError> {
        Ok(GreetReply {
            text: greeting.text(),
        })
    }
}

/// A STOMP endpoint seeding the identity from the CONNECT login, claiming it when `claim` says.
fn stomp(claim: bool) -> StompConfig {
    let config = StompConfig::default().with_authenticator(|connect: StompConnect| async move {
        let subject = connect
            .login()
            .ok_or_else(|| StompAuthenticationError::new("a login is required"))?
            .to_owned();

        Ok(
            StompAuthenticated::new(StompPrincipal::authenticated(subject.as_str()))
                .seed(Identity { subject }),
        )
    });

    match claim {
        true => config.provides_seed::<Identity>(),
        false => config,
    }
}

/// The builder with the endpoint and, when `seed_requests` says, the request seeder.
fn builder(
    environment: &TestEnvironment,
    seed_requests: bool,
    claim_connections: bool,
) -> upwell::AppBuilder<upwell::axum::Axum> {
    let builder = IdentityApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws_with::<Stomp>("/stomp", stomp(claim_connections));

    match seed_requests {
        true => builder.request_seed::<Identity, HeaderIdentity>(),
        false => builder,
    }
}

/// The status line and body of a raw `GET path` with `headers`.
async fn get(address: std::net::SocketAddr, path: &str, headers: &str) -> (String, String) {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("the server accepts");
    let request =
        format!("GET {path} HTTP/1.1\r\nhost: test\r\n{headers}connection: close\r\n\r\n");
    let mut response = String::new();

    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is sent");
    stream
        .read_to_string(&mut response)
        .await
        .expect("the response is read");

    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head.lines().next().unwrap_or_default().to_owned();

    (status, body.to_owned())
}

#[tokio::test]
async fn one_component_reads_the_identity_over_rest_and_stomp() {
    let environment = TestEnvironment::new("upwell-http-context-identity-");
    let app = builder(&environment, true, true)
        .request_seed::<Identity, HeaderIdentity>()
        .build()
        .await
        .expect("both seeds are claimed, and registering a seeder twice is a no-op");
    let server = TestServer::start_with_guard(app, environment).await;
    let address = server.address();

    let (status, body) = deadline(
        "greet over REST",
        get(address, "/greet", "x-user: alice\r\n"),
    )
    .await;

    assert!(status.contains("200"), "{status}");
    assert!(body.contains("hello alice"), "{body}");

    let (rejected, _) = deadline("greet without a user", get(address, "/greet", "")).await;

    assert!(rejected.contains("401"), "the seeder rejects: {rejected}");

    let connection = deadline(
        "STOMP connect",
        StompClientTransport::connect_with_options(
            &format!("ws://{address}/stomp"),
            StompConnectOptions::new().with_login("bob"),
        ),
    )
    .await
    .expect("connects");
    let reply = deadline(
        "greet over STOMP",
        StompGreetingClient::new(connection.clone()).greet(GreetAsk {}),
    )
    .await
    .expect("a reply");

    assert_eq!(reply.text, "hello bob");

    deadline("disconnect", connection.disconnect())
        .await
        .expect("disconnect cleanly");
    server.shutdown().await;
}

#[tokio::test]
async fn an_unclaimed_request_identity_fails_the_build() {
    let environment = TestEnvironment::new("upwell-http-context-unclaimed-request-");

    let Err(error) = builder(&environment, false, true).build().await else {
        panic!("the request placement of the identity is unclaimed");
    };
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(message.contains("HttpRequest"), "{message}");
}

#[tokio::test]
async fn an_unclaimed_connection_identity_fails_the_build() {
    let environment = TestEnvironment::new("upwell-http-context-unclaimed-connection-");

    let Err(error) = builder(&environment, true, false).build().await else {
        panic!("the connection placement of the identity is unclaimed");
    };
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(message.contains("WebsocketConnection"), "{message}");
}

#[tokio::test]
async fn two_seeders_of_one_type_fail_the_build() {
    let environment = TestEnvironment::new("upwell-http-context-two-seeders-");

    let Err(error) = builder(&environment, true, true)
        .request_seed::<Identity, AnonymousIdentity>()
        .build()
        .await
    else {
        panic!("two seeders would fill each request scope twice");
    };
    let message = error.to_string();

    assert!(message.contains("HeaderIdentity"), "{message}");
    assert!(message.contains("AnonymousIdentity"), "{message}");
}

#[tokio::test]
async fn an_endpoint_that_does_not_seed_the_identity_fails_the_build() {
    let environment = TestEnvironment::new("upwell-http-context-bare-endpoint-");

    let Err(error) = builder(&environment, true, true)
        .register_ws::<JsonWs>("/json")
        .build()
        .await
    else {
        panic!("the STOMP endpoint's seed does not vouch for the JSON-WS connections");
    };
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(
        message.contains("/json"),
        "the endpoint that lacks it is named: {message}"
    );
}

#[tokio::test]
async fn a_claimed_seed_without_an_authenticator_fails_the_build() {
    let environment = TestEnvironment::new("upwell-http-context-no-authenticator-");

    let Err(error) = IdentityApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .request_seed::<Identity, HeaderIdentity>()
        .register_ws_with::<Stomp>("/stomp", StompConfig::default().provides_seed::<Identity>())
        .build()
        .await
    else {
        panic!("nothing would produce the claimed identity");
    };
    let message = format!("{error:?}");

    assert!(message.contains("SeedWithoutAuthenticator"), "{message}");
}
