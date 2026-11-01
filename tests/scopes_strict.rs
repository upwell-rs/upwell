#![cfg(feature = "axum")]

//! End-to-end tests for `deny_unplanned`: a request scope opened for a route builds on
//! demand only what the route declares.
//!
//! Path middleware extracts a request-scoped component from the route's request scope. On a
//! route whose handler injects that component the extraction is declared; on a route whose
//! handler injects nothing it is drift, which an app built with `deny_unplanned` rejects and
//! any other app builds on demand.

use std::sync::Arc;

use upwell::axum::axum::Router;
use upwell::axum::axum::body::Body;
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::axum::middleware::Next;
use upwell::axum::axum::response::{IntoResponse, Response};
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{AxumMiddleware, RequestMeta, ScopeHandle};
use upwell::{component, methods};

/// A request-scoped component the declared route injects.
#[component(scope = HttpRequest)]
struct Audit {}

#[methods]
impl Audit {
    #[init]
    async fn init(_meta: RequestMeta) -> Self {
        Self {}
    }
}

/// Path middleware extracting [`Audit`] from the request scope, answering
/// `500 Internal Server Error` when the scope refuses to build it.
#[component]
struct ReadAudit {}

impl AxumMiddleware for ReadAudit {
    async fn handle(&self, mut req: RawRequest, next: Next) -> Response {
        let scope = ScopeHandle::of(&mut req)
            .await
            .expect("the middleware opens the request scope");

        match scope.0.extract::<Arc<Audit>>().await {
            Ok(_) => next.run(req).await,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

/// Routes reading [`Audit`] through [`ReadAudit`], declared by one handler and not the other.
#[controller(path = "/audit")]
struct AuditController;

#[handlers]
impl AuditController {
    #[get("/declared", middleware = [ReadAudit])]
    async fn declared(&self, Inject(_audit): Inject<Arc<Audit>>) {}

    #[get("/drift", middleware = [ReadAudit])]
    async fn drift(&self) {}
}

/// The router of an app, denying undeclared on-demand builds when `deny` is set.
async fn router(deny: bool) -> Router {
    let builder = upwell::App::<upwell::axum::Axum>::builder("test-strict").auto_discover();
    let builder = match deny {
        true => builder.deny_unplanned(),
        false => builder,
    };
    let app = builder.build().await.expect("app builds");

    app.protocol().router().clone()
}

/// The status `router` answers a GET to `uri` with.
async fn status(router: &Router, uri: &'static str) -> StatusCode {
    let request = RawRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request builds");

    router
        .clone()
        .oneshot(request)
        .await
        .expect("the router answers")
        .status()
}

#[tokio::test]
async fn a_strict_app_builds_only_what_a_route_declares() {
    let router = router(true).await;

    let declared = status(&router, "/audit/declared").await;
    let drift = status(&router, "/audit/drift").await;

    assert_eq!(declared, StatusCode::OK);
    assert_eq!(
        drift,
        StatusCode::INTERNAL_SERVER_ERROR,
        "middleware reading a component the route does not declare is rejected"
    );
}

#[tokio::test]
async fn other_apps_build_undeclared_components_on_demand() {
    let router = router(false).await;

    let drift = status(&router, "/audit/drift").await;

    assert_eq!(drift, StatusCode::OK);
}
