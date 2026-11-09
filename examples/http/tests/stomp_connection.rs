//! STOMP opens a connection's scope only after `CONNECT` authentication (#126): the
//! authenticator resolves its arguments from the application root, its seeds fill the
//! connection scope, and no connection-scoped component is built for a rejected connection.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::axum::prelude::*;
use upwell::axum::{
    CodecError, StompAuthenticationError, StompClientTransport, StompConnectOptions,
    WebsocketConnectionScope,
};
use upwell::prelude::*;
use upwell::{ComponentDescriptor, Scope, TypeDescriptor};
use upwell_test_utils::{TestEnvironment, TestServer, deadline};

app! {
    /// Generated host shared by the STOMP connection tests.
    app StompConnectionApplication {
        name: "stomp-connection-test",
        protocol: upwell::axum::Axum,
    }
}

/// How many [`ConnectionAudit`]s were built, across every connection of the binary.
static AUDITS_BUILT: AtomicUsize = AtomicUsize::new(0);

/// An app-defined tenant the authenticator seeds into the connection scope.
#[derive(Clone, Debug)]
struct Tenant(String);

impl Injectable for Tenant {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

impl upwell::Provided<Tenant> for upwell::Wiring {}

/// The connection scope's seed slot for [`Tenant`].
static TENANT: ComponentDescriptor = ComponentDescriptor::manual(
    "tenant",
    "Tenant",
    TypeDescriptor::of::<Tenant>("Tenant"),
    WebsocketConnectionScope::KEY,
);

/// A connection-scoped component built on every opening of the connection scope, so it records
/// what an accepted connection looks like the moment it opens.
#[component(scope = WebsocketConnection, eager)]
struct ConnectionAudit {
    principal: StompPrincipal,
    tenant: Tenant,
}

#[methods]
impl ConnectionAudit {
    #[init]
    async fn init(principal: StompPrincipal, tenant: Tenant) -> Self {
        AUDITS_BUILT.fetch_add(1, Ordering::SeqCst);

        Self { principal, tenant }
    }
}

/// A request a client sends to learn who the connection is.
#[dto]
struct WhoAsk {}

/// Who the connection is, as its connection-scoped audit saw it.
#[dto]
struct WhoAmI {
    subject: Option<String>,
    tenant: String,
}

/// A STOMP controller answering who the connection is.
#[controller(ws = Stomp)]
struct Who {}

#[handlers(ws = Stomp)]
impl Who {
    #[message("/app/whoami")]
    async fn whoami(
        &self,
        _ask: WhoAsk,
        Inject(audit): Inject<Arc<ConnectionAudit>>,
    ) -> Result<WhoAmI, CodecError> {
        Ok(WhoAmI {
            subject: audit.principal.subject().map(str::to_owned),
            tenant: audit.tenant.0.clone(),
        })
    }
}

/// A trait nothing provides, so an authenticator injecting it fails at `CONNECT`.
trait Directory: Send + Sync {}

/// An authenticator accepting `alice` / `secret` and seeding the tenant from the upgrade's
/// `x-tenant` header.
async fn tenant_authenticator(
    connect: StompConnect,
) -> Result<StompAuthenticated, StompAuthenticationError> {
    let tenant = connect
        .upgrade()
        .headers
        .get("x-tenant")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("none")
        .to_owned();

    if connect.login() != Some("alice") || connect.passcode() != Some("secret") {
        return Err(StompAuthenticationError::new("invalid credentials"));
    }

    Ok(StompAuthenticated::new(StompPrincipal::authenticated("alice")).seed(Tenant(tenant)))
}

/// Builds the test app with `config` on `/stomp`.
async fn build(
    environment: &TestEnvironment,
    config: StompConfig,
) -> Result<upwell::App<upwell::axum::Axum>, impl std::fmt::Debug> {
    StompConnectionApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .component_descriptor(&TENANT)
        .register_ws_with::<Stomp>("/stomp", config)
        .build()
        .await
}

#[tokio::test]
async fn the_connection_scope_opens_after_authentication_with_its_seeds() {
    let environment = TestEnvironment::new("upwell-http-stomp-connection-");
    let app = build(
        &environment,
        StompConfig::default().with_authenticator(tenant_authenticator),
    )
    .await
    .expect("app builds");
    let server = TestServer::start_with_guard(app, environment).await;
    let url = format!("ws://{}/stomp", server.address());
    let built_before = AUDITS_BUILT.load(Ordering::SeqCst);

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

    assert!(rejected.is_err(), "invalid credentials are rejected");
    assert_eq!(
        AUDITS_BUILT.load(Ordering::SeqCst),
        built_before,
        "a rejected connection builds no connection-scoped component"
    );

    let connection = deadline(
        "authenticated STOMP connect",
        StompClientTransport::connect_with_options(
            &url,
            StompConnectOptions::new()
                .with_login("alice")
                .with_passcode("secret"),
        ),
    )
    .await
    .expect("valid credentials connect");
    let who = deadline(
        "whoami",
        WhoClient::new(connection.clone()).whoami(WhoAsk {}),
    )
    .await
    .expect("a reply");

    assert_eq!(who.subject.as_deref(), Some("alice"));
    assert_eq!(who.tenant, "none");
    assert_eq!(AUDITS_BUILT.load(Ordering::SeqCst), built_before + 1);

    deadline("disconnect", connection.disconnect())
        .await
        .expect("disconnect cleanly");
    server.shutdown().await;
}

#[tokio::test]
async fn an_authenticator_injecting_a_connection_component_fails_the_build() {
    let environment = TestEnvironment::new("upwell-http-stomp-connection-scoped-auth-");
    let config = StompConfig::default().with_authenticator(
        |_connect: StompConnect, _audit: Arc<ConnectionAudit>| async move {
            Ok(StompPrincipal::anonymous())
        },
    );

    let Err(error) = build(&environment, config).await else {
        panic!("an authenticator injecting a connection-scoped component is rejected");
    };
    let message = format!("{error:?}");

    assert!(
        message.contains("ScopedAuthenticatorDependency"),
        "{message}"
    );
    assert!(message.contains("ConnectionAudit"), "{message}");
}

#[tokio::test]
async fn an_unresolvable_authenticator_argument_rejects_the_connection() {
    let environment = TestEnvironment::new("upwell-http-stomp-unresolved-auth-");
    let config = StompConfig::default().with_authenticator(
        |_connect: StompConnect, _directory: Arc<dyn Directory>| async move {
            Ok(StompPrincipal::anonymous())
        },
    );
    let app = build(&environment, config)
        .await
        .expect("a trait argument is resolved at CONNECT time");
    let server = TestServer::start_with_guard(app, environment).await;
    let url = format!("ws://{}/stomp", server.address());

    let rejected = deadline(
        "unresolved-authenticator STOMP connect",
        StompClientTransport::connect(&url),
    )
    .await;

    assert!(
        rejected.is_err(),
        "an argument the root cannot resolve rejects the connection"
    );

    server.shutdown().await;
}
