//! Bytes allocated by one whole reload transaction over large graphs, by kind of change.
//!
//! The scenarios match `reload_transition`, but report allocation traffic instead of
//! wall-clock time: the figure is deterministic for a given build, so it tracks how much
//! whole-graph work a reload does even on a loaded machine. Like `reload_transition`, a
//! size-proportional figure for the `unrelated` scenario is preparation work a narrower
//! change could skip.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Builder;
use upwell_benchmarks::alloc::{self, TrackingAllocator};
use upwell_benchmarks::measure::AllocBytes;
use upwell_benchmarks::reload::{Change, ReloadApp};

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

const SIZES: [usize; 3] = [256, 1024, 2048];

const CHANGES: [(&str, Change); 3] = [
    ("none", Change::None),
    ("unrelated", Change::Unrelated),
    ("condition", Change::Condition),
];

fn reload(c: &mut Criterion<AllocBytes>) {
    let runtime = Builder::new_current_thread()
        .build()
        .expect("bench runtime starts");

    for (label, change) in CHANGES {
        let mut group = c.benchmark_group(format!("reload_allocations/{label}"));

        for size in SIZES {
            let mut app = runtime.block_on(ReloadApp::build(label, size));

            group.throughput(Throughput::Elements(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
                bencher.iter_custom(|iterations| {
                    let mut bytes = 0;

                    for _ in 0..iterations {
                        app.stage(change);

                        let ((), delta) = alloc::measure(|| runtime.block_on(app.reload()));

                        bytes += delta.bytes_allocated;
                    }

                    bytes
                });
            });
        }

        group.finish();
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .with_measurement(AllocBytes)
        .without_plots()
        .sample_size(10);
    targets = reload
}
criterion_main!(benches);
