#![allow(dead_code)]

//! A request seed reaches only controller routes (#128): a request-scoped component no
//! controller route reaches, such as one only global middleware injects, may be built for a
//! request no route matches or a WebSocket upgrade, which opens the request scope without the
//! seed, so preparation rejects its requirement.

use upwell::axum::RequestSeeder;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::axum::http::request::Parts;
use upwell::axum::axum::response::{IntoResponse, Response};
use upwell::axum::prelude::*;
use upwell::prelude::*;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for the request-opener test.
    app RequestOpenersApplication {
        name: "request-openers-test",
        protocol: upwell::axum::Axum,
    }
}

/// Who a request runs as.
#[context(scopes = [HttpRequest])]
#[derive(Clone)]
struct Identity {
    subject: String,
}

/// Seeds the identity from the `x-user` header.
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

/// What global middleware would inject: no controller route reaches it.
#[component(scope = HttpRequest)]
struct MiddlewareView {
    identity: Identity,
}

#[tokio::test]
async fn a_component_no_controller_route_reaches_cannot_require_a_request_seed() {
    let environment = TestEnvironment::new("upwell-http-request-openers-");

    let Err(error) = RequestOpenersApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .request_seed::<Identity, HeaderIdentity>()
        .build()
        .await
    else {
        panic!("a request no route matches opens the scope without the identity");
    };
    let message = error.to_string();

    assert!(message.contains("MiddlewareView"), "{message}");
    assert!(
        message.contains("axum non-controller requests"),
        "{message}"
    );
}
