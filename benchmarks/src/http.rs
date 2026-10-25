//! In-process HTTP fixtures for measuring Upwell's per-request cost against raw axum.
//!
//! Every fixture is an [`axum::Router`] driven through `tower::ServiceExt::oneshot`, so a bench
//! measures routing, middleware and DI work without sockets. The Upwell fixtures are real
//! applications built through `App::<Axum>`, so they include the request-scope layer every route
//! runs behind.
//!
//! Each case builds its own application, because the request scope currently constructs every
//! component registered in it: the component count is part of the case, not a shared backdrop.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt;
use upwell_app::{App, AppRuntime};
use upwell_axum::scope::HttpRequest;
use upwell_axum::{Axum, AxumAppBuilder, ControllerDescriptor, Inject};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{Scope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, Injectable,
};

/// The route every fixture serves without touching DI.
pub const HELLO: &str = "/hello";

/// The route that injects one request-scoped component.
pub const SCOPED: &str = "/scoped";

/// The largest number of request-scoped components a fixture can register.
pub const MAX_SCOPED: usize = 64;

/// A zero-sized request-scoped component, distinct per `N`, so its cost is the container's alone.
pub struct RequestScoped<const N: usize>;

impl<const N: usize> Component for RequestScoped<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "request-scoped-component";
    const NAME: &'static str = "RequestScopedComponent";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// The type token naming the benchmark controller.
struct BenchController;

fn construct<const N: usize>(
    _cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<RequestScoped<N>>(RequestScoped::<N>::NAME),
            value: Box::new(Injectable::into_stored(RequestScoped::<N>.into_handle())),
        })
    })
}

fn factories<const N: usize>() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct: construct::<N>,
            dependencies: Vec::new,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn request_scoped<const N: usize>() -> ComponentDescriptor {
    ComponentDescriptor {
        id: Box::leak(format!("request-scoped-component-{N}").into_boxed_str()),
        scope: HttpRequest::KEY,
        factories: factories::<N>,
        ..ComponentDescriptor::of::<RequestScoped<N>>()
    }
}

macro_rules! scoped_roster {
    ($($n:literal),* $(,)?) => {
        vec![ $( request_scoped::<$n>() ),* ]
    };
}

/// Every request-scoped roster descriptor, in index order.
fn roster() -> &'static [ComponentDescriptor] {
    static ROSTER: OnceLock<Vec<ComponentDescriptor>> = OnceLock::new();

    ROSTER.get_or_init(|| {
        scoped_roster!(
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
        )
    })
}

async fn hello() -> &'static str {
    "hello"
}

async fn scoped(Inject(_component): Inject<Arc<RequestScoped<0>>>) -> &'static str {
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
    assert!(
        scoped <= MAX_SCOPED,
        "fixture needs {scoped} request-scoped components but the roster holds {MAX_SCOPED}"
    );

    let builder = App::<Axum>::builder("http-request-bench")
        .config_source(ConfigManager::<Dynamic>::empty())
        .controller_descriptor(&CONTROLLER);
    let builder = roster()[..scoped]
        .iter()
        .fold(builder, |builder, descriptor| {
            builder.component_descriptor(descriptor)
        });

    builder.build().await.expect("benchmark app builds")
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
