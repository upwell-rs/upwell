//! One HTTP request through the axum router, in process.

use std::sync::Arc;

use upwell::axum::axum::Router;
use upwell::axum::axum::body::Body;
use upwell::axum::axum::extract::DefaultBodyLimit;
use upwell::axum::axum::http::{Request, StatusCode};
use upwell::axum::axum::routing::get;
use upwell::axum::tower::ServiceExt;
use upwell_app::App;
use upwell_axum::scope::HttpRequest;
use upwell_axum::{
    Axum, AxumAppBuilder, ControllerDescriptor, HttpOutputDescriptor, HttpOutputShape,
    HttpRouteDescriptor, Inject,
};
use upwell_config::{ConfigManager, Toml};
use upwell_core::{DependencyDescriptor, EntryKey, Scope, TypeDescriptor};
use upwell_di::FromContainer;

use crate::alloc::Delta;
use crate::roster::{Scoped, UNUSED, per_call, roster, runtime};

/// The route that injects nothing.
const HELLO: &str = "/hello";

/// The route that injects one request-scoped component.
const SCOPED: &str = "/scoped";

/// A type token naming the controller serving [`HELLO`].
struct HelloController;

/// A type token naming the controller serving [`SCOPED`].
struct ScopedController;

async fn hello() -> &'static str {
    "hello"
}

async fn scoped(Inject(_component): Inject<Arc<Scoped<0>>>) -> &'static str {
    "scoped"
}

fn hello_router(_runtime: &upwell_app::AppRuntime) -> Router {
    Router::new().route(HELLO, get(hello))
}

fn scoped_router(_runtime: &upwell_app::AppRuntime) -> Router {
    Router::new().route(SCOPED, get(scoped))
}

/// The route at `path`, injecting `targets`.
fn route(
    handler: &'static str,
    path: &'static str,
    key: &'static str,
    targets: fn() -> Vec<DependencyDescriptor>,
) -> HttpRouteDescriptor {
    HttpRouteDescriptor {
        handler,
        method: "GET",
        path,
        path_parameters: &[],
        inputs: &[],
        output: HttpOutputDescriptor {
            ty: None,
            declared: "&'static str",
            shape: HttpOutputShape::Unary,
            responses: &[],
        },
        source: None,
        key: EntryKey::new(key).expect("contract entry keys are valid"),
        targets,
    }
}

fn hello_routes() -> Vec<HttpRouteDescriptor> {
    vec![route("hello", HELLO, "http/contract.hello", Vec::new)]
}

fn scoped_routes() -> Vec<HttpRouteDescriptor> {
    vec![route("scoped", SCOPED, "http/contract.scoped", || {
        vec![<Arc<Scoped<0>>>::dependency()]
    })]
}

static HELLO_CONTROLLER: ControllerDescriptor = ControllerDescriptor {
    id: "contract-hello-controller",
    name: "HelloController",
    ty: TypeDescriptor::of::<HelloController>("HelloController"),
    base: "",
    router: hello_router,
    routes: hello_routes,
};

/// Registered only with `Scoped<0>`, which its route injects.
static SCOPED_CONTROLLER: ControllerDescriptor = ControllerDescriptor {
    id: "contract-scoped-controller",
    name: "ScopedController",
    ty: TypeDescriptor::of::<ScopedController>("ScopedController"),
    base: "",
    router: scoped_router,
    routes: scoped_routes,
};

/// The protocol's request timeout is off, so the only policy layer a request runs through
/// besides the request scope is the body limit, which [`raw_router`] applies too.
const CONFIG: &str = "[axum]\nrequest_timeout_ms = 0\n";

/// The router of an Upwell app serving [`HELLO`] and [`SCOPED`] with `count` request-scoped
/// components, and the body limit it applies.
async fn upwell_router(count: usize) -> (Router, usize) {
    let builder = App::<Axum>::builder("http-allocation-contracts")
        .config_source(ConfigManager::<Toml>::from_str(CONFIG).expect("the contract config parses"))
        .controller_descriptor(&HELLO_CONTROLLER);
    let builder = match count {
        0 => builder,
        _ => builder.controller_descriptor(&SCOPED_CONTROLLER),
    };
    let app = roster(HttpRequest::KEY, count)
        .iter()
        .fold(builder, |builder, component| {
            builder.component_descriptor(component)
        })
        .build()
        .await
        .expect("the contract app builds");

    (
        app.protocol().router().clone(),
        app.protocol().config().max_request_body_bytes,
    )
}

/// [`HELLO`] on raw axum, behind the same body limit the Upwell router applies.
fn raw_router(body_limit: usize) -> Router {
    Router::new()
        .route(HELLO, get(hello))
        .layer(DefaultBodyLimit::max(body_limit))
}

async fn get_ok(router: &Router, path: &'static str) -> bool {
    let request = Request::get(path)
        .body(Body::empty())
        .expect("the contract request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("axum routers are infallible");

    response.status() == StatusCode::OK
}

/// The Upwell router of an app with `count` request-scoped components and the raw axum router
/// with the same body limit, on one runtime.
struct Routers {
    runtime: tokio::runtime::Runtime,
    upwell: Router,
    raw: Router,
}

impl Routers {
    fn new(count: usize) -> Self {
        let runtime = runtime();
        let (upwell, body_limit) = runtime.block_on(upwell_router(count));

        Self {
            runtime,
            upwell,
            raw: raw_router(body_limit),
        }
    }

    /// What one request to `path` on the Upwell router allocates.
    fn upwell(&self, path: &'static str) -> Delta {
        per_call(&self.runtime, || get_ok(&self.upwell, path))
    }

    /// What one request to [`HELLO`] on raw axum allocates.
    fn raw(&self) -> Delta {
        per_call(&self.runtime, || get_ok(&self.raw, HELLO))
    }
}

/// "DI bytes" are what a request allocates beyond the same route on raw axum behind the same
/// policy layers: the Upwell router's body limit, with the request timeout off.
///
/// Not met yet, so ignored: on a route injecting nothing, the Upwell router makes 5 more
/// allocations and 560 more bytes per request than raw axum (25 and 2 301 against 20 and
/// 1 741, measured over 32 calls and divided). Four allocations and about 400 bytes are axum
/// re-boxing every route for the request-scope layer, which `PreparedAxum::build` applies as
/// its own `Router::layer`: any layer, `tower::layer::util::Identity` included, costs that. The
/// remaining allocation and bytes are not attributed yet.
#[test]
#[ignore = "the request-scope layer costs 5 allocations and 560 bytes per request; see the docs"]
fn a_route_injecting_nothing_allocates_no_more_than_raw_axum() {
    let routers = Routers::new(UNUSED);

    assert_eq!(
        routers.upwell(HELLO),
        routers.raw(),
        "a route injecting nothing allocated DI bytes"
    );
}

#[test]
fn unused_request_scoped_components_cost_a_request_nothing() {
    let without = Routers::new(0).upwell(HELLO);
    let with_unused = Routers::new(UNUSED).upwell(HELLO);

    assert_eq!(
        with_unused, without,
        "{UNUSED} unused request-scoped components changed what a request injecting nothing allocates"
    );
}

#[test]
fn a_route_injecting_one_component_costs_its_plan_not_the_scope() {
    let alone = Routers::new(1).upwell(SCOPED);
    let among_unused = Routers::new(1 + UNUSED).upwell(SCOPED);

    assert_eq!(
        among_unused, alone,
        "{UNUSED} unused request-scoped components changed what a request injecting one allocates"
    );
}
