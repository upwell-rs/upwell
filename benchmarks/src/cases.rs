//! A shared Criterion harness for per-call benches measured in both time and heap bytes.
//!
//! A bench describes its cases as [`Case`]s over a [`Call`] and hands them to [`time`] and
//! [`bytes`]. Both check that every case succeeds before measuring, so no case times an error
//! path, and both use the same Criterion settings, so every per-call bench reports alike.

use std::future::Future;
use std::hint::black_box;
use std::time::Duration;

use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Criterion};
use tokio::runtime::{Builder, Runtime};

use crate::measure::AllocBytes;

/// One unit of measured work, such as a request or a message, reporting whether it succeeded.
pub trait Call {
    /// Performs the call once and returns whether it succeeded.
    fn call(&self) -> impl Future<Output = bool>;
}

/// A named benchmark case.
pub struct Case<C> {
    name: String,
    call: C,
}

impl<C> Case<C> {
    /// Names `call` as one case of a bench group.
    pub fn new(name: impl Into<String>, call: C) -> Self {
        Self {
            name: name.into(),
            call,
        }
    }
}

/// Criterion settings for the wall-clock group of a per-call bench.
pub fn time_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .sample_size(100)
}

/// Criterion settings for the heap-bytes group of a per-call bench.
pub fn bytes_config() -> Criterion<AllocBytes> {
    Criterion::default()
        .with_measurement(AllocBytes)
        .without_plots()
        .sample_size(10)
}

/// Measures the wall-clock time of every case built by `cases`, on a multi-threaded runtime.
pub fn time<C: Call>(c: &mut Criterion, group: &str, cases: impl FnOnce(&Runtime) -> Vec<Case<C>>) {
    let runtime = Runtime::new().expect("tokio runtime");
    let cases = cases(&runtime);

    run(c.benchmark_group(group), &runtime, &cases);
}

/// Measures the heap bytes allocated by every case built by `cases`. A single-threaded runtime
/// keeps other threads' allocations out of the per-call figure.
pub fn bytes<C: Call>(
    c: &mut Criterion<AllocBytes>,
    group: &str,
    cases: impl FnOnce(&Runtime) -> Vec<Case<C>>,
) {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let cases = cases(&runtime);

    run(c.benchmark_group(group), &runtime, &cases);
}

fn run<M: Measurement, C: Call>(
    mut group: BenchmarkGroup<'_, M>,
    runtime: &Runtime,
    cases: &[Case<C>],
) {
    for case in cases {
        assert!(
            runtime.block_on(case.call.call()),
            "benchmark case `{}` fails before measurement",
            case.name
        );
    }

    for case in cases {
        group.bench_function(case.name.as_str(), |bencher| {
            bencher
                .to_async(runtime)
                .iter(|| async { black_box(case.call.call().await) });
        });
    }

    group.finish();
}
