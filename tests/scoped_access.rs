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
use upwell::context::{ContextError, Current, ScopedError};
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

/// What one request saw of its ticket.
#[dto]
struct Seen {
    /// The ticket the handler injected.
    ticket: usize,
    /// The singleton desk's read in the handler.
    direct: usize,
    /// The desk's read from a carried tokio task.
    spawned: usize,
    /// The desk's read from a carried blocking task.
    blocking: usize,
    /// Whether a plain `tokio::spawn` saw no context.
    plain_saw_nothing: bool,
}

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
        let spawned = upwell::context::tokio::spawn(async move { spawned_desk.number().await })
            .await
            .expect("the carried task completes")
            .expect("the carried task reads the ticket");
        let handle = tokio::runtime::Handle::current();
        let blocking =
            upwell::context::tokio::spawn_blocking(move || handle.block_on(desk.number()))
                .await
                .expect("the blocking task completes")
                .expect("the blocking task reads the ticket");
        let plain = tokio::spawn(async { Current::<Arc<Ticket>>::new().get().await.err() })
            .await
            .expect("the plain task completes");

        Json(Seen {
            ticket: ticket.number,
            direct,
            spawned,
            blocking,
            plain_saw_nothing: matches!(
                plain,
                Some(ScopedError::Context(ContextError::OutsideContext))
            ),
        })
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
        assert_eq!(
            seen.direct, seen.ticket,
            "the singleton reads the request's ticket"
        );
        assert_eq!(
            seen.spawned, seen.ticket,
            "a carried task reads the same ticket"
        );
        assert_eq!(
            seen.blocking, seen.ticket,
            "a carried blocking task reads it too"
        );
        assert!(seen.plain_saw_nothing, "a plain spawn carries nothing");
    }

    assert_ne!(
        first.ticket, second.ticket,
        "each request has its own ticket"
    );
}
