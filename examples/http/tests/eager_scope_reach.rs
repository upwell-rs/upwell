#![allow(dead_code)]

//! An eager component reaches only the placements its own scope's openings build (#128): an
//! eager component of an application scope does not make the WebSocket placement of a shared
//! type depend on every endpoint.

use std::sync::Arc;

use upwell::axum::prelude::*;
use upwell::axum::{CodecError, StompAuthenticationError};
use upwell::prelude::*;
use upwell::scope::Root;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for the scoped eager-reach test.
    app EagerScopeReachApplication {
        name: "eager-scope-reach-test",
        protocol: upwell::axum::Axum,
    }
}

/// A background job the application opens itself.
#[scope(parents = [Root])]
pub struct Job;

/// Who a job or a connection runs as.
#[context(scopes = [Job, WebsocketConnection])]
#[derive(Clone)]
struct Identity {
    subject: String,
}

/// Needs the identity, per job and per connection.
#[component(scope = [Job, WebsocketConnection])]
struct Keyring {
    identity: Identity,
}

/// Built on every job opening with the job's keyring.
#[component(scope = Job, eager)]
struct JobGate {
    keyring: Arc<Keyring>,
}

/// A STOMP request for the keyring's subject.
#[dto]
struct Ask {}

/// The STOMP controller, the only entry that reaches the connection's keyring.
#[controller(ws = Stomp)]
struct StompKeyring {}

#[handlers(ws = Stomp)]
impl StompKeyring {
    #[message("/app/keyring")]
    async fn keyring(
        &self,
        _ask: Ask,
        Inject(keyring): Inject<Arc<Keyring>>,
    ) -> Result<String, CodecError> {
        Ok(keyring.identity.subject.clone())
    }
}

/// A JSON-WS controller that injects nothing.
#[controller(ws = JsonWs)]
struct JsonPing;

#[handlers(ws = JsonWs)]
impl JsonPing {
    #[message("ping")]
    async fn ping(&self) -> u32 {
        0
    }
}

#[tokio::test]
async fn an_eager_job_component_does_not_reach_the_connection_placement() {
    let environment = TestEnvironment::new("upwell-http-eager-scope-reach-");
    let stomp = StompConfig::default()
        .with_authenticator(|connect: StompConnect| async move {
            let subject = connect
                .login()
                .ok_or_else(|| StompAuthenticationError::new("a login is required"))?
                .to_owned();

            Ok(
                StompAuthenticated::new(StompPrincipal::authenticated(subject.as_str()))
                    .seed(Identity { subject }),
            )
        })
        .provides_seed::<Identity>();

    EagerScopeReachApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .provides_seed::<Identity, Job>()
        .register_ws_with::<Stomp>("/stomp", stomp)
        .register_ws::<JsonWs>("/json")
        .build()
        .await
        .expect("JSON-WS handlers never reach the connection's keyring");
}
