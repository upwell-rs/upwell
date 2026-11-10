#![allow(dead_code)]

//! An eager connection component builds on every endpoint's connections, and so does what it
//! depends on (#128): a value that component chain requires must be seeded by every endpoint,
//! even when a STOMP handler is the only entry that injects the component directly.

use std::sync::Arc;

use upwell::axum::prelude::*;
use upwell::axum::{CodecError, StompAuthenticationError};
use upwell::prelude::*;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for the eager-reach test.
    app EagerReachApplication {
        name: "eager-reach-test",
        protocol: upwell::axum::Axum,
    }
}

/// Who a connection runs as, seeded by the STOMP authenticator only.
#[context(scopes = [WebsocketConnection])]
#[derive(Clone)]
struct Identity {
    subject: String,
}

/// Records the connection's identity.
#[component(scope = WebsocketConnection)]
struct Audit {
    identity: Identity,
}

/// Built on every connection opening, of every endpoint, with its audit.
#[component(scope = WebsocketConnection, eager)]
struct Gate {
    audit: Arc<Audit>,
}

/// A STOMP request for the audited subject.
#[dto]
struct Ask {}

/// The STOMP controller, the only entry that injects the audit directly.
#[controller(ws = Stomp)]
struct StompAudit {}

#[handlers(ws = Stomp)]
impl StompAudit {
    #[message("/app/audit")]
    async fn audit(
        &self,
        _ask: Ask,
        Inject(audit): Inject<Arc<Audit>>,
    ) -> Result<String, CodecError> {
        Ok(audit.identity.subject.clone())
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
async fn an_eager_components_dependencies_need_every_endpoints_seed() {
    let environment = TestEnvironment::new("upwell-http-eager-reach-");
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

    let Err(error) = EagerReachApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws_with::<Stomp>("/stomp", stomp)
        .register_ws::<JsonWs>("/json")
        .build()
        .await
    else {
        panic!("JSON-WS connections build the gate and its audit without an identity");
    };
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(message.contains("/json"), "{message}");
}
