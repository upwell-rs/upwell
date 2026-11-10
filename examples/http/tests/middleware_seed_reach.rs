#![allow(dead_code)]

//! DI-backed global middleware runs for every request, also one no route matches, whose
//! request scope opens without the request seeds (#128): a component the middleware reaches over
//! a declared edge must not require a request seed, even when a controller handler reaches it
//! too.

use std::sync::Arc;

use upwell::axum::axum::extract::Request;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::axum::http::request::Parts;
use upwell::axum::axum::middleware::Next;
use upwell::axum::axum::response::{IntoResponse, Response};
use upwell::axum::prelude::*;
use upwell::axum::{AxumMiddleware, RequestSeeder};
use upwell::context::Current;
use upwell::prelude::*;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for the middleware-reach test.
    app MiddlewareReachApplication {
        name: "middleware-seed-reach-test",
        protocol: upwell::axum::Axum,
    }
}

/// Who a request runs as, seeded for controller routes only.
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

/// The request's audit log, which needs its identity.
#[component(scope = HttpRequest)]
struct AuditLog {
    identity: Identity,
}

/// Global middleware reading the request's audit log.
#[component]
struct AuditMiddleware {
    audit: Current<Arc<AuditLog>>,
}

impl AxumMiddleware for AuditMiddleware {
    async fn handle(&self, request: Request, next: Next) -> Response {
        let _ = self.audit.get().await;

        next.run(request).await
    }
}

/// A controller whose handler reads the audit log too.
#[controller(path = "/audit")]
struct AuditController {}

#[handlers]
impl AuditController {
    #[get("/")]
    async fn audit(&self, Inject(audit): Inject<Arc<AuditLog>>) -> String {
        audit.identity.subject.clone()
    }
}

#[tokio::test]
async fn a_component_global_middleware_reaches_cannot_require_a_request_seed() {
    let environment = TestEnvironment::new("upwell-http-middleware-seed-reach-");

    let Err(error) = MiddlewareReachApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .request_seed::<Identity, HeaderIdentity>()
        .middleware::<AuditMiddleware>()
        .build()
        .await
    else {
        panic!("a request no route matches runs the middleware without the identity");
    };
    let message = error.to_string();

    assert!(message.contains("AuditLog"), "{message}");
    assert!(
        message.contains("axum non-controller requests"),
        "{message}"
    );
}
