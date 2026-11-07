#![cfg(feature = "axum")]

//! End-to-end tests for an HTTP route that injects only a singleton whose `Current<T>` reads a
//! request-scoped value built from the request head: the route's entry plans what the accessor
//! reads, so the head is captured and the value may be built, also under `deny_unplanned`.

use std::sync::Arc;

use upwell::axum::axum::Router;
use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::StatusCode;
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
