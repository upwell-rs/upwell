#![cfg(feature = "axum")]

//! End-to-end tests for the invocation context of HTTP requests.
//!
//! Every request served by the protocol's router runs, global middleware included, in one
//! context. The request scope may open after the context, on first injection, and joins it when
//! it opens; with eager request-scoped components it opens before global middleware runs.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use upwell::axum::axum::Router;
use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::{HeaderValue, StatusCode};
use upwell::axum::axum::middleware::{self as axum_middleware, Next};
use upwell::axum::axum::response::Response;
use upwell::axum::prelude::*;
use upwell::axum::tower::{Layer, Service, ServiceExt};
use upwell::axum::{Axum, ScopeHandle};
use upwell::{AppBuilder, Scope, component, methods};
use upwell_di::ScopeContainer;
use upwell_di::context::{Invocation, InvocationContext};

/// The header middleware reports what it saw in.
const MIDDLEWARE: &str = "x-middleware";

/// An eager request-scoped component.
#[component(scope = HttpRequest, eager)]
struct Eager {}

#[methods]
impl Eager {
    #[init]
    async fn init() -> Self {
        Self {}
    }
}

/// Routes reporting the context their handler runs in.
#[controller(path = "/context")]
struct ContextController;

#[handlers]
impl ContextController {
    #[get("/none")]
    async fn none(&self) -> String {
        describe(None)
    }

    #[get("/handle")]
    async fn handle(&self, ScopeHandle(scope): ScopeHandle) -> String {
        tokio::task::yield_now().await;

        describe(Some(&scope))
    }
}

/// `http` when the active context is an HTTP request's, then the scope it runs in: `none`
/// without one, `same` when it is `expected`, or else the scope's name.
fn describe(expected: Option<&Arc<ScopeContainer>>) -> String {
    let context = InvocationContext::capture().expect("the request runs in a context");
    let kind = match context.invocation() {
        Invocation::HttpRequest => "http",
        _ => "other",
    };

    let scope = match (context.scope_key(), expected) {
        (None, _) => "none",
        (Some(_), Some(expected)) if context.is_scope(expected) => "same",
        (Some(key), _) if key.id() == HttpRequest::ID => "request",
        (Some(_), _) => "other",
    };

    format!("{kind}:{scope}")
}

/// Middleware recording the context before and after it opens the request scope.
async fn observe(mut request: RawRequest, next: Next) -> Response {
    let before = describe(None);
    let ScopeHandle(scope) = ScopeHandle::of(&mut request)
        .await
        .expect("the middleware opens the request scope");
    let after = describe(Some(&scope));
    let mut response = next.run(request).await;
    let seen = HeaderValue::from_str(&format!("{before},{after}")).expect("ascii header");

    response.headers_mut().insert(MIDDLEWARE, seen);

    response
}

/// A hand-written layer recording the context of its service's synchronous `call`.
#[derive(Clone, Default)]
struct RecordCall {
    seen: Arc<Mutex<Option<String>>>,
}

/// The service [`RecordCall`] wraps.
#[derive(Clone)]
struct RecordCallService<S> {
    inner: S,
    seen: Arc<Mutex<Option<String>>>,
}

impl<S> Layer<S> for RecordCall {
    type Service = RecordCallService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RecordCallService {
            inner,
            seen: Arc::clone(&self.seen),
        }
    }
}

impl<S: Service<RawRequest>> Service<RawRequest> for RecordCallService<S> {
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RawRequest) -> Self::Future {
        *self.seen.lock().expect("the record is healthy") = Some(describe(None));

        self.inner.call(request)
    }
}

fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-context-http").controller::<ContextController>()
}

async fn router(builder: AppBuilder<Axum>) -> Router {
    let app = builder.build().await.expect("app builds");

    app.protocol().router().clone()
}

/// The status, body and middleware report of a `GET` to `uri`.
async fn get(router: &Router, uri: &str) -> (StatusCode, String, Option<String>) {
    let request = RawRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request is valid");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    let status = response.status();
    let middleware = response
        .headers()
        .get(MIDDLEWARE)
        .map(|value| value.to_str().expect("ascii header").to_owned());
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads");

    (
        status,
        String::from_utf8(bytes.to_vec()).expect("utf-8 body"),
        middleware,
    )
}

#[tokio::test]
async fn a_handler_runs_in_the_request_context_and_its_scope_once_opened() {
    let router = router(app()).await;

    assert_eq!(
        get(&router, "/context/none").await,
        (StatusCode::OK, "http:none".to_owned(), None),
        "a request injecting nothing opens no scope"
    );
    assert_eq!(
        get(&router, "/context/handle").await,
        (StatusCode::OK, "http:same".to_owned(), None),
        "the scope the handler opened joins the context"
    );
}

#[tokio::test]
async fn middleware_and_handler_share_the_request_scope_middleware_opened() {
    let router = router(app().layer(axum_middleware::from_fn(observe))).await;

    assert_eq!(
        get(&router, "/context/handle").await,
        (
            StatusCode::OK,
            "http:same".to_owned(),
            Some("http:none,http:same".to_owned())
        )
    );
    assert_eq!(
        get(&router, "/context/none").await,
        (
            StatusCode::OK,
            "http:request".to_owned(),
            Some("http:none,http:same".to_owned())
        ),
        "a handler injecting nothing still runs in the scope middleware opened"
    );
}

#[tokio::test]
async fn an_eager_request_scope_is_in_the_context_before_global_middleware() {
    let router = router(
        app()
            .component::<Eager>()
            .layer(axum_middleware::from_fn(observe)),
    )
    .await;

    assert_eq!(
        get(&router, "/context/none").await,
        (
            StatusCode::OK,
            "http:request".to_owned(),
            Some("http:request,http:same".to_owned())
        )
    );
}

#[tokio::test]
async fn a_tower_service_runs_its_synchronous_call_in_the_context() {
    let record = RecordCall::default();
    let router = router(app().layer(record.clone())).await;

    get(&router, "/context/none").await;

    assert_eq!(
        record
            .seen
            .lock()
            .expect("the record is healthy")
            .as_deref(),
        Some("http:none")
    );
}

#[tokio::test]
async fn concurrent_requests_see_only_their_own_scope() {
    let router = router(app().layer(axum_middleware::from_fn(observe))).await;
    let mut requests = tokio::task::JoinSet::new();

    for _ in 0..32 {
        let router = router.clone();

        requests.spawn(async move { get(&router, "/context/handle").await });
    }

    while let Some(response) = requests.join_next().await {
        assert_eq!(
            response.expect("the request task completes"),
            (
                StatusCode::OK,
                "http:same".to_owned(),
                Some("http:none,http:same".to_owned())
            )
        );
    }
}

#[test]
fn code_outside_every_request_has_no_context() {
    assert!(InvocationContext::capture().is_err());
}
