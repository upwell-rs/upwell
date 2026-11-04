#![cfg(feature = "axum")]

//! End-to-end tests for declared access to an HTTP request's scope from a singleton, and for
//! carrying the request's context into spawned work.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::axum::Axum;
use upwell::axum::axum::Router;
use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::context::{CapturedContext, ContextError, Current, ScopedError};
use upwell::{AppBuilder, component, methods};

/// Numbers handed out to [`Ticket`]s.
static ISSUED: AtomicUsize = AtomicUsize::new(0);

/// A request-scoped component numbered per request.
#[component(scope = HttpRequest)]
struct Ticket {
    #[default]
    number: usize,
}

#[methods]
impl Ticket {
    #[init]
    async fn init() -> Self {
        Self {
            number: ISSUED.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// A singleton reading the request's ticket on each call.
#[component]
struct Desk {
    ticket: Current<Arc<Ticket>>,
}

impl Desk {
    async fn number(&self) -> Result<usize, ScopedError> {
        Ok(self.ticket.get().await?.number)
    }
}

/// What a request saw: its own ticket, the desk's read, a carried task's read, a carried
/// blocking task's read, and `1` when a plain task saw no context.
type Seen = Vec<usize>;

/// Routes reading the ticket in-line and from spawned work.
#[controller(path = "/ticket")]
struct TicketController;

#[handlers]
impl TicketController {
    #[get("/read")]
    async fn ticket(
        &self,
        Inject(ticket): Inject<Arc<Ticket>>,
        Inject(desk): Inject<Arc<Desk>>,
    ) -> Json<Seen> {
        let direct = desk.number().await.expect("the desk reads the ticket");
        let spawned_desk = Arc::clone(&desk);
        let spawned = upwell::context::spawn(async move { spawned_desk.number().await })
            .await
            .expect("the carried task completes")
            .expect("the carried task reads the ticket");
        let handle = tokio::runtime::Handle::current();
        let blocking = CapturedContext::capture()
            .expect("the handler runs in a context")
            .spawn_blocking(move || handle.block_on(desk.number()))
            .await
            .expect("the blocking task completes")
            .expect("the blocking task reads the ticket");
        let plain = tokio::spawn(async { Current::<Arc<Ticket>>::new().get().await.err() })
            .await
            .expect("the plain task completes");
        let plain_saw_nothing = matches!(
            plain,
            Some(ScopedError::Context(ContextError::OutsideContext))
        );

        Json(vec![
            ticket.number,
            direct,
            spawned,
            blocking,
            usize::from(plain_saw_nothing),
        ])
    }
}

async fn router() -> Router {
    let app = upwell::App::<Axum>::builder("test-scoped-access")
        .controller::<TicketController>()
        .component::<Ticket>()
        .component::<Desk>();
    let app = AppBuilder::build(app).await.expect("app builds");

    app.protocol().router().clone()
}

async fn seen(router: &Router) -> Seen {
    let request = RawRequest::builder()
        .uri("/ticket/read")
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
async fn a_singleton_reads_each_requests_own_value_and_spawned_work_carries_it() {
    let router = router().await;

    let (first, second) = tokio::join!(seen(&router), seen(&router));

    for seen in [&first, &second] {
        let [ticket, direct, spawned, blocking, plain] = seen[..] else {
            panic!("the handler reports five readings, got {seen:?}");
        };

        assert_eq!(direct, ticket, "the singleton reads the request's ticket");
        assert_eq!(spawned, ticket, "a carried task reads the same ticket");
        assert_eq!(blocking, ticket, "a carried blocking task reads it too");
        assert_eq!(plain, 1, "a plain spawn carries nothing");
    }

    assert_ne!(first[0], second[0], "each request has its own ticket");
}
