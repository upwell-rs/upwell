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

use std::hint::black_box;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use tokio::runtime::{Builder, Runtime};
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::http::{self, HELLO, SCOPED};
use upwell_benchmarks::measure::AllocBytes;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

/// Unused request-scoped component counts; 10 matches the regression report in #343.
const UNUSED: [usize; 3] = [1, 10, 64];

/// One benchmark case: a router and the path it is called on.
struct Case {
    name: String,
    router: Router,
    path: &'static str,
}

/// Builds `count` request-scoped components into an app and takes its assembled router.
async fn upwell_router(count: usize) -> Router {
    let app = http::upwell_app(count).await;

    app.protocol().router().clone()
}

/// Builds every case, checking each answers `200 OK` so no case measures an error path.
fn cases(runtime: &Runtime) -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "raw_axum".to_string(),
            router: http::raw_router(),
            path: HELLO,
        },
        Case {
            name: "upwell_hello".to_string(),
            router: runtime.block_on(upwell_router(0)),
            path: HELLO,
        },
        Case {
            name: "upwell_scoped".to_string(),
            router: runtime.block_on(upwell_router(1)),
            path: SCOPED,
        },
    ];

    for count in UNUSED {
        cases.push(Case {
            name: format!("upwell_unused/{count}"),
            router: runtime.block_on(upwell_router(count)),
            path: HELLO,
        });
    }

    for case in &cases {
        let status = runtime.block_on(http::get_status(&case.router, case.path));

        assert_eq!(
            status,
            StatusCode::OK,
            "{} answers {}",
            case.name,
            case.path
        );
    }

    cases
}

fn run_cases<M: Measurement>(group: &mut BenchmarkGroup<'_, M>, runtime: &Runtime, cases: &[Case]) {
    for case in cases {
        group.bench_function(case.name.as_str(), |bencher| {
            bencher
                .to_async(runtime)
                .iter(|| async { black_box(http::get_status(&case.router, case.path).await) });
        });
    }
}

fn request_time(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let cases = cases(&runtime);
    let mut group = c.benchmark_group("http_request");

    run_cases(&mut group, &runtime, &cases);

    group.finish();
}

/// A single-threaded runtime keeps other threads' allocations out of the per-request figure.
fn request_bytes(c: &mut Criterion<AllocBytes>) {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let cases = cases(&runtime);
    let mut group = c.benchmark_group("http_request_bytes");

    run_cases(&mut group, &runtime, &cases);

    group.finish();
}

criterion_group! {
    name = time;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .sample_size(100);
    targets = request_time
}

criterion_group! {
    name = bytes;
    config = Criterion::default()
        .with_measurement(AllocBytes)
        .without_plots()
        .sample_size(10);
    targets = request_bytes
}

criterion_main!(time, bytes);
