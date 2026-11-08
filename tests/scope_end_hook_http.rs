#![cfg(feature = "axum")]

//! End-to-end test that a protocol's request scope ends through `ScopeEnded`: an HTTP request
//! opens its scope, and dropping the last handle to it, once the response is built, ends it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use upwell::axum::axum::body::Body;
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{Axum, HttpRequest};
use upwell::{AppBuilder, Scope, ScopeEnd, ScopeEndKind, ScopeEnded, component, methods};

/// Every end the recorder saw.
static ENDS: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// A component of the request scope, so a route injecting it opens the scope.
#[component(scope = HttpRequest)]
struct RequestState {}

/// One route injecting the request-scoped component.
#[controller(path = "/ping")]
struct PingController;

#[handlers]
impl PingController {
    #[get("/")]
    async fn ping(&self, Inject(_state): Inject<Arc<RequestState>>) {}
}

/// Observes every ended instance.
#[component]
struct Recorder {}

#[methods]
impl Recorder {
    #[hook(ScopeEnded)]
    async fn ended(&self, end: ScopeEnd) {
        ENDS.lock().expect("the ends lock is healthy").push(end);
    }
}

fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-scope-end-http")
        .controller::<PingController>()
        .component::<RequestState>()
        .component::<Recorder>()
}

#[tokio::test]
async fn a_request_scope_ends_once_the_request_is_done() {
    let app = app().build().await.expect("app builds");
    let request = RawRequest::builder()
        .uri("/ping")
        .body(Body::empty())
        .expect("the request is valid");
    let response = app
        .protocol()
        .router()
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");

    assert_eq!(response.status(), StatusCode::OK);

    drop(response);

    let ends = request_ends().await;

    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].kind, ScopeEndKind::Released);
    assert!(ends[0].params.is_empty());
}

/// The request scope ends recorded once one arrived, awaited while the observers run on their
/// own task.
async fn request_ends() -> Vec<ScopeEnd> {
    for _ in 0..2500 {
        let ends = ends_of_requests();

        if !ends.is_empty() {
            return ends;
        }

        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    panic!("the request scope never ended");
}

fn ends_of_requests() -> Vec<ScopeEnd> {
    ENDS.lock()
        .expect("the ends lock is healthy")
        .iter()
        .filter(|end| end.scope == HttpRequest::ID)
        .cloned()
        .collect()
}
