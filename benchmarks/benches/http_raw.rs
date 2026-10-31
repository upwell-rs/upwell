//! The framework comparison's raw axum baseline: every route hand-wired, no framework layer.
//!
//! See [`upwell_benchmarks::frameworks`] for the routes and groups shared with `http_summer` and
//! `http_upwell`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::cases;
use upwell_benchmarks::frameworks::{self, ALL_ROUTES, DEP, HELLO, SCOPED, Server};
use upwell_benchmarks::measure::AllocBytes;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

/// The shared singleton, held as router state.
#[derive(Clone, Default)]
struct Greeter(Arc<AtomicU64>);

/// The per-request value, built by hand in the handler.
struct Ticket {
    id: u64,
}

async fn hello() -> &'static str {
    "hello"
}

async fn dep(State(greeter): State<Greeter>) -> String {
    format!("n={}", greeter.0.fetch_add(1, Ordering::Relaxed))
}

async fn scoped() -> String {
    let ticket = Arc::new(Ticket { id: 0 });

    format!("t={}", ticket.id)
}

fn servers(_runtime: &Runtime) -> Vec<Server> {
    let router = Router::new()
        .route(HELLO, get(hello))
        .route(DEP, get(dep))
        .route(SCOPED, get(scoped))
        .with_state(Greeter::default());

    vec![Server::new("raw", router, ALL_ROUTES)]
}

fn request_time(c: &mut Criterion) {
    frameworks::request_time(c, servers);
}

fn request_bytes(c: &mut Criterion<AllocBytes>) {
    frameworks::request_bytes(c, servers);
}

fn loopback(c: &mut Criterion) {
    frameworks::loopback(c, servers);
}

criterion_group! {
    name = time;
    config = cases::time_config();
    targets = request_time, loopback
}

criterion_group! {
    name = bytes;
    config = cases::bytes_config();
    targets = request_bytes
}

criterion_main!(time, bytes);
