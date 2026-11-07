#![cfg(feature = "axum")]

//! End-to-end tests for an HTTP route that injects only a singleton whose `Current<T>` reads a
//! request-scoped value built from the request head: the route's entry plans what the accessor
//! reads, so the head is captured and the value may be built, also under `deny_unplanned`.

use std::sync::Arc;

use upwell::axum::axum::Router;
use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::{HeaderValue, StatusCode};
use upwell::axum::axum::middleware::{Next, from_fn};
use upwell::axum::axum::response::Response;
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{Axum, RequestMeta};
use upwell::context::{Current, ScopedError};
use upwell::{AppBuilder, DiError, Dynamic, component, methods};

/// The header the request's identity comes from.
const USER: &str = "x-user";

/// A request-scoped identity built from the request head.
#[component(scope = HttpRequest)]
struct Identity {
    #[default]
    user: Option<String>,
}

#[methods]
impl Identity {
    #[init]
    async fn init(meta: RequestMeta) -> Self {
        Self {
            user: meta
                .headers
                .get(USER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        }
    }
}

/// A singleton reading the request's identity on each call.
#[component]
struct Desk {
    identity: Current<Arc<Identity>>,
}

impl Desk {
    async fn user(&self) -> Result<Option<String>, ScopedError> {
        Ok(self.identity.get().await?.user.clone())
    }
}

/// The response header middleware reports the user its desk read in.
const SEEN: &str = "x-seen";

/// A request-scoped desk reading the request's identity on each call, which only middleware
/// injects.
#[component(scope = HttpRequest)]
struct RequestDesk {
    identity: Current<Arc<Identity>>,
}

/// A request-scoped component that reads nothing of the request.
#[component(scope = HttpRequest)]
struct Unrelated {}

/// Middleware injecting the request desk and reporting the user it reads.
async fn read_desk(
    Inject(desk): Inject<Arc<RequestDesk>>,
    request: RawRequest,
    next: Next,
) -> Response {
    let user = user_of(&desk.identity).await;

    report(next.run(request).await, &user)
}

/// An eager request-scoped desk reading the request's identity, which only middleware
/// injects.
#[component(scope = HttpRequest, eager)]
struct EagerDesk {
    identity: Current<Arc<Identity>>,
}

/// The user `identity` holds, or an empty string.
async fn user_of(identity: &Current<Arc<Identity>>) -> String {
    identity
        .get()
        .await
        .expect("the middleware reads the identity")
        .user
        .clone()
        .unwrap_or_default()
}

/// Middleware injecting the already built singleton [`Desk`] and reporting the user it reads.
async fn read_singleton_desk(
    Inject(desk): Inject<Arc<Desk>>,
    request: RawRequest,
    next: Next,
) -> Response {
    let user = user_of(&desk.identity).await;

    report(next.run(request).await, &user)
}

/// Middleware injecting the eager [`EagerDesk`] and reporting the user it reads.
async fn read_eager_desk(
    Inject(desk): Inject<Arc<EagerDesk>>,
    request: RawRequest,
    next: Next,
) -> Response {
    let user = user_of(&desk.identity).await;

    report(next.run(request).await, &user)
}

/// `response` with `user` reported in [`SEEN`].
fn report(mut response: Response, user: &str) -> Response {
    response.headers_mut().insert(
        SEEN,
        HeaderValue::from_str(user).expect("the user is a header value"),
    );

    response
}

/// A type nothing provides.
struct Absent;

/// A request-scoped component whose construction fails: nothing provides its dynamic
/// dependency.
#[component(scope = HttpRequest)]
struct Broken {
    _absent: Dynamic<Arc<Absent>>,
}

/// A request-scoped component no application registers.
#[component(scope = HttpRequest)]
struct Unregistered {}

/// A singleton reading a target that fails to build and one nothing provides.
#[component]
struct Probe {
    broken: Current<Arc<Broken>>,
    unregistered: Current<Option<Arc<Unregistered>>>,
}

/// A route injecting only the singleton.
#[controller(path = "/desk")]
struct DeskController;

#[handlers]
impl DeskController {
    #[get("/user")]
    async fn user(&self, Inject(desk): Inject<Arc<Desk>>) -> Json<Option<String>> {
        Json(desk.user().await.expect("the desk reads the identity"))
    }

    #[get("/unrelated")]
    async fn unrelated(&self, Inject(_unrelated): Inject<Arc<Unrelated>>) {}

    #[get("/probe")]
    async fn probe(&self, Inject(probe): Inject<Arc<Probe>>) -> Json<(bool, bool)> {
        let broken = probe.broken.get().await;
        let unregistered = probe.unregistered.get().await;
        let construction_failed = matches!(
            broken,
            Err(ScopedError::Resolution { source, .. })
                if matches!(*source, DiError::MissingComponent(_))
        );

        Json((construction_failed, matches!(unregistered, Ok(None))))
    }
}

fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-scoped-access-planned")
        .controller::<DeskController>()
        .component::<Identity>()
        .component::<Desk>()
        .component::<Broken>()
        .component::<Probe>()
        .component::<RequestDesk>()
        .component::<Unrelated>()
}

async fn router(app: AppBuilder<Axum>) -> Router {
    let app = AppBuilder::build(app).await.expect("app builds");

    app.protocol().router().clone()
}

async fn get<T: serde::de::DeserializeOwned>(router: &Router, uri: &str) -> T {
    let request = RawRequest::builder()
        .uri(uri)
        .header(USER, "ada")
        .body(Body::empty())
        .expect("the request is valid");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads");

    serde_json::from_slice(&body).expect("the body decodes")
}

async fn user(router: &Router) -> Option<String> {
    get(router, "/desk/user").await
}

#[tokio::test]
async fn an_accessor_reads_a_value_built_from_the_captured_head() {
    let router = router(app()).await;

    assert_eq!(user(&router).await.as_deref(), Some("ada"));
}

#[tokio::test]
async fn an_accessor_read_is_declared_under_deny_unplanned() {
    let router = router(app().deny_unplanned()).await;

    assert_eq!(user(&router).await.as_deref(), Some("ada"));
}

#[tokio::test]
async fn a_target_that_fails_to_build_keeps_its_source_apart_from_an_absent_one() {
    let router = router(app()).await;

    let (construction_failed, absent_is_none) = get::<(bool, bool)>(&router, "/desk/probe").await;

    assert!(
        construction_failed,
        "a missing dependency of the target is a resolution failure with its source"
    );
    assert!(
        absent_is_none,
        "an optional target nothing provides is none"
    );
}

/// What the middleware `layer` reported for a request to a route that injects only
/// [`Unrelated`].
async fn seen_by_middleware(app: AppBuilder<Axum>) -> String {
    let router = router(app).await;
    let request = RawRequest::builder()
        .uri("/desk/unrelated")
        .header(USER, "ada")
        .body(Body::empty())
        .expect("the request is valid");
    let response = router
        .oneshot(request)
        .await
        .expect("the router is infallible");

    assert_eq!(response.status(), StatusCode::OK);

    response
        .headers()
        .get(SEEN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_default()
}

#[tokio::test]
async fn middleware_injecting_an_accessor_owner_the_route_never_reaches_captures_the_head() {
    let app = app().layer(from_fn(read_desk));

    assert_eq!(seen_by_middleware(app).await, "ada");
}

#[tokio::test]
async fn middleware_injecting_a_built_singleton_with_an_accessor_captures_the_head() {
    let app = app().layer(from_fn(read_singleton_desk));

    assert_eq!(seen_by_middleware(app).await, "ada");
}

#[tokio::test]
async fn an_eager_accessor_owner_reads_a_declared_value_under_deny_unplanned() {
    let app = app()
        .component::<EagerDesk>()
        .deny_unplanned()
        .layer(from_fn(read_eager_desk));

    assert_eq!(seen_by_middleware(app).await, "ada");
}

/// Whether an eager-only opening of the request scope of `app` binds the request head.
async fn eager_opening_binds_the_head(app: AppBuilder<Axum>) -> bool {
    let app = AppBuilder::build(app).await.expect("app builds");
    let opened = app
        .runtime()
        .opener::<HttpRequest>()
        .expect("the request scope is declared")
        .builder(HttpRequest)
        .open_with_bound_seed(upwell_app::Opening::OnDemand, || {
            RequestMeta::from_parts(
                upwell::axum::axum::http::Method::GET,
                upwell::axum::axum::http::Uri::from_static("/"),
                upwell::axum::axum::http::HeaderMap::new(),
            )
        })
        .await
        .expect("the request scope opens");

    opened
        .container()
        .resolve::<RequestMeta>()
        .await
        .ok()
        .flatten()
        .is_some()
}

#[tokio::test]
async fn an_eager_only_opening_binds_the_seeds_an_accessor_reads_and_only_then() {
    let accessed = app();
    let plain = upwell::App::<Axum>::builder("test-scoped-access-plain").component::<Identity>();

    assert!(
        eager_opening_binds_the_head(accessed).await,
        "the accessor's identity reads the head"
    );
    assert!(
        !eager_opening_binds_the_head(plain).await,
        "an application without accessors copies no head"
    );
}
