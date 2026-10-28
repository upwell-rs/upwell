//! In-process HTTP fixtures for measuring Upwell's per-request cost against raw axum.
//!
//! Every fixture is an [`axum::Router`] driven through `tower::ServiceExt::oneshot`, so a bench
//! measures routing, middleware and DI work without sockets. The Upwell fixtures are real
//! applications built through `App::<Axum>`, so they include the request-scope layer every route
//! runs behind.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt;
use upwell_app::{App, AppRuntime};
use upwell_axum::scope::HttpRequest;
use upwell_axum::{Axum, AxumAppBuilder, ControllerDescriptor, Inject};
use upwell_core::{Scope, TypeDescriptor};

use crate::cases::Call;
use crate::scoped::{self, CallScoped};

/// The route every fixture serves without touching DI.
pub const HELLO: &str = "/hello";

/// The route that injects one request-scoped component.
pub const SCOPED: &str = "/scoped";

/// The type token naming the benchmark controller.
struct BenchController;

async fn hello() -> &'static str {
    "hello"
}

async fn scoped(Inject(_component): Inject<Arc<CallScoped<0>>>) -> &'static str {
    "scoped"
}

fn bench_routes(_runtime: &AppRuntime) -> Router {
    Router::new()
        .route(HELLO, get(hello))
        .route(SCOPED, get(scoped))
}

static CONTROLLER: ControllerDescriptor = ControllerDescriptor {
    id: "bench-controller",
    name: "BenchController",
    ty: TypeDescriptor::of::<BenchController>("BenchController"),
    base: "",
    router: bench_routes,
    routes: Vec::new,
};

/// The raw axum baseline: the same [`HELLO`] route with no Upwell layer at all.
pub fn raw_router() -> Router {
    Router::new().route(HELLO, get(hello))
}

/// An Upwell application serving [`HELLO`] and [`SCOPED`], with the first `scoped` roster
/// components registered in the request scope. [`SCOPED`] needs `scoped >= 1`.
pub async fn upwell_app(scoped: usize) -> App<Axum> {
    let builder = scoped::builder::<Axum>("http-request-bench").controller_descriptor(&CONTROLLER);

    scoped::register(builder, HttpRequest::KEY, scoped)
        .build()
        .await
        .expect("benchmark app builds")
}

/// Sends one `GET path` through `router` in process and returns the response status.
pub async fn get_status(router: &Router, path: &'static str) -> StatusCode {
    let request = Request::get(path)
        .body(Body::empty())
        .expect("benchmark request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("axum routers are infallible");

    response.status()
}

/// One `GET path` through `router`, succeeding on `200 OK`.
pub struct HttpCall {
    router: Router,
    path: &'static str,
}

impl HttpCall {
    /// Calls `path` on `router`.
    pub fn new(router: Router, path: &'static str) -> Self {
        Self { router, path }
    }
}

impl Call for HttpCall {
    async fn call(&self) -> bool {
        get_status(&self.router, self.path).await == StatusCode::OK
    }
}
