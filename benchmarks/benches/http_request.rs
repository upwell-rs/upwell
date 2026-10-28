//! One HTTP request through the axum router, in process, against a raw axum baseline.
//!
//! Every case serves the same trivial handler, so the gap between a case and `raw_axum` is the
//! framework's per-request cost:
//!
//! - `raw_axum`: a plain axum router, no Upwell layer.
//! - `upwell_hello`: an Upwell route that injects nothing, in an app with no request-scoped
//!   components.
//! - `upwell_scoped`: a route injecting one request-scoped component, the only one registered.
//! - `upwell_unused/N`: the injection-free route in an app with `N` request-scoped components
//!   that the route never uses.
//!
//! The same cases run twice: `http_request` measures wall-clock time, and `http_request_bytes`
//! measures heap bytes allocated per request, which is deterministic and runner-independent.
//! Per-request cost should track what a route uses, so a widening gap between `upwell_hello`
//! and `upwell_unused/N` is a regression.

use axum::Router;
use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::cases::{self, Case};
use upwell_benchmarks::http::{self, HELLO, HttpCall, SCOPED};
use upwell_benchmarks::measure::AllocBytes;
use upwell_benchmarks::scoped::UNUSED;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

/// Builds `count` request-scoped components into an app and takes its assembled router.
async fn upwell_router(count: usize) -> Router {
    let app = http::upwell_app(count).await;

    app.protocol().router().clone()
}

fn http_cases(runtime: &Runtime) -> Vec<Case<HttpCall>> {
    let upwell = |count, path| HttpCall::new(runtime.block_on(upwell_router(count)), path);
    let mut cases = vec![
        Case::new("raw_axum", HttpCall::new(http::raw_router(), HELLO)),
        Case::new("upwell_hello", upwell(0, HELLO)),
        Case::new("upwell_scoped", upwell(1, SCOPED)),
    ];

    for count in UNUSED {
        cases.push(Case::new(
            format!("upwell_unused/{count}"),
            upwell(count, HELLO),
        ));
    }

    cases
}

fn request_time(c: &mut Criterion) {
    cases::time(c, "http_request", http_cases);
}

fn request_bytes(c: &mut Criterion<AllocBytes>) {
    cases::bytes(c, "http_request_bytes", http_cases);
}

criterion_group! {
    name = time;
    config = cases::time_config();
    targets = request_time
}

criterion_group! {
    name = bytes;
    config = cases::bytes_config();
    targets = request_bytes
}

criterion_main!(time, bytes);
