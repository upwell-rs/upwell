#![cfg(feature = "axum")]

//! End-to-end tests for eager request-scoped components over HTTP.
//!
//! An eager component runs for every routed request that global middleware lets through, whether
//! or not the route injects anything. Each test registers what it needs by hand, so an eager
//! component of one test does not reach the routes of another.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::axum::axum::Router;
use upwell::axum::axum::body::Body;
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::{HeaderValue, StatusCode};
use upwell::axum::axum::middleware::{self as axum_middleware, Next};
use upwell::axum::axum::response::{IntoResponse, Response};
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{Axum, RequestMeta, ScopeHandle};
use upwell::{AppBuilder, component, methods};

static SPANS: AtomicUsize = AtomicUsize::new(0);
static TENANT: Mutex<Option<String>> = Mutex::new(None);

/// An eager request-scoped component counting the requests that built it.
#[component(scope = HttpRequest, eager)]
struct Span {}

#[methods]
impl Span {
    #[init]
    async fn init() -> Self {
        SPANS.fetch_add(1, Ordering::SeqCst);

        Self {}
    }
}

/// An eager request-scoped component whose constructor always fails.
#[component(scope = HttpRequest, eager)]
struct Broken;

#[methods]
impl Broken {
    #[init]
    async fn init() -> Result<Self, std::io::Error> {
        Err(std::io::Error::other("the broken component cannot build"))
    }
}

/// An eager request-scoped component recording the `x-tenant` header it reads from the request.
#[component(scope = HttpRequest, eager)]
struct TenantSpan {}

#[methods]
impl TenantSpan {
    #[init]
    async fn init(meta: RequestMeta) -> Self {
        let tenant = meta
            .headers
            .get("x-tenant")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        *TENANT.lock().expect("the tenant lock is healthy") = tenant;

        Self {}
    }
}

/// One route injecting nothing and one injecting the eager component.
#[controller(path = "/eager")]
struct EagerController;

#[handlers]
impl EagerController {
    #[get("/none")]
    async fn none(&self) {}

    #[get("/injecting")]
    async fn injecting(&self, Inject(_span): Inject<Arc<Span>>) {}
}

fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-eager-http")
        .controller::<EagerController>()
        .component::<Span>()
}

async fn router(builder: AppBuilder<Axum>) -> Router {
    let app = builder.build().await.expect("app builds");

    app.protocol().router().clone()
}

async fn get(router: &Router, uri: &str) -> StatusCode {
    let request = RawRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request is valid");

    router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible")
        .status()
}

/// Middleware answering every request itself.
async fn reject(_request: RawRequest, _next: Next) -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

/// Middleware opening the request scope, then answering the request itself.
async fn inject_then_reject(mut request: RawRequest, _next: Next) -> Response {
    ScopeHandle::of(&mut request)
        .await
        .expect("the middleware opens the request scope");

    StatusCode::UNAUTHORIZED.into_response()
}

/// Middleware opening the request scope, then continuing.
async fn inject(mut request: RawRequest, next: Next) -> Response {
    ScopeHandle::of(&mut request)
        .await
        .expect("the middleware opens the request scope");

    next.run(request).await
}

/// Middleware setting the `x-tenant` header.
async fn set_tenant(mut request: RawRequest, next: Next) -> Response {
    request
        .headers_mut()
        .insert("x-tenant", HeaderValue::from_static("acme"));

    next.run(request).await
}

/// Runs `request` and returns how many [`Span`]s it built, with its status.
async fn spans_built(router: &Router, uri: &str) -> (usize, StatusCode) {
    let before = SPANS.load(Ordering::SeqCst);
    let status = get(router, uri).await;

    (SPANS.load(Ordering::SeqCst) - before, status)
}

#[tokio::test]
async fn an_eager_component_runs_once_for_every_routed_request() {
    let router = router(app()).await;

    assert_eq!(
        spans_built(&router, "/eager/none").await,
        (1, StatusCode::OK),
        "a route injecting nothing runs it"
    );
    assert_eq!(
        spans_built(&router, "/eager/injecting").await,
        (1, StatusCode::OK),
        "a route injecting it shares the instance the opening built"
    );
    assert_eq!(
        spans_built(&router, "/eager/nowhere").await,
        (0, StatusCode::NOT_FOUND),
        "a request no route matches opens no scope"
    );
}

#[tokio::test]
async fn a_failing_eager_component_fails_a_route_injecting_nothing() {
    let router = router(app().component::<Broken>()).await;

    assert_eq!(
        get(&router, "/eager/none").await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn global_middleware_that_answers_keeps_the_eager_component_from_running() {
    let router = router(app().layer(axum_middleware::from_fn(reject))).await;

    assert_eq!(
        spans_built(&router, "/eager/none").await,
        (0, StatusCode::UNAUTHORIZED)
    );
}

#[tokio::test]
async fn global_middleware_that_injects_opens_the_one_scope_of_the_request() {
    let injecting = router(app().layer(axum_middleware::from_fn(inject))).await;
    let answering = router(app().layer(axum_middleware::from_fn(inject_then_reject))).await;

    assert_eq!(
        spans_built(&injecting, "/eager/injecting").await,
        (1, StatusCode::OK),
        "the scope middleware opened is the one the route uses"
    );
    assert_eq!(
        spans_built(&answering, "/eager/none").await,
        (1, StatusCode::UNAUTHORIZED),
        "opening the scope runs the eager component"
    );
}

#[tokio::test]
async fn an_eager_component_reads_the_head_global_middleware_left() {
    let router = router(
        app()
            .component::<TenantSpan>()
            .layer(axum_middleware::from_fn(set_tenant)),
    )
    .await;

    assert_eq!(get(&router, "/eager/none").await, StatusCode::OK);
    assert_eq!(
        TENANT
            .lock()
            .expect("the tenant lock is healthy")
            .as_deref(),
        Some("acme")
    );
}
