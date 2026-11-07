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
use upwell::{AppBuilder, component, methods};

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

/// A route injecting only the singleton.
#[controller(path = "/desk")]
struct DeskController;

#[handlers]
impl DeskController {
    #[get("/user")]
    async fn user(&self, Inject(desk): Inject<Arc<Desk>>) -> Json<Option<String>> {
        Json(desk.user().await.expect("the desk reads the identity"))
    }
}

fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-scoped-access-planned")
        .controller::<DeskController>()
        .component::<Identity>()
        .component::<Desk>()
}

async fn router(app: AppBuilder<Axum>) -> Router {
    let app = AppBuilder::build(app).await.expect("app builds");

    app.protocol().router().clone()
}

async fn user(router: &Router) -> Option<String> {
    let request = RawRequest::builder()
        .uri("/desk/user")
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
