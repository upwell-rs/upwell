//! Wall-clock cost of one whole reload transaction over large graphs, by kind of change.
//!
//! - `none` — the config file is unchanged.
//! - `unrelated` — a value no condition or component reads changes.
//! - `condition` — one condition fact flips, enabling or disabling one leaf component.
//!
//! The roster nodes never depend on config, so every scenario retains them all. Any cost
//! that grows with graph size in the `none` and `unrelated` scenarios is whole-graph
//! preparation work a narrower change could skip. The config rewrite runs outside the
//! timed region.

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use upwell_benchmarks::reload::{Change, ReloadApp};

const SIZES: [usize; 3] = [256, 1024, 2048];

const CHANGES: [(&str, Change); 3] = [
    ("none", Change::None),
    ("unrelated", Change::Unrelated),
    ("condition", Change::Condition),
];

fn reload(c: &mut Criterion) {
    let runtime = Runtime::new().expect("bench runtime starts");

    for (label, change) in CHANGES {
        let mut group = c.benchmark_group(format!("reload_transition/{label}"));

        for size in SIZES {
            let mut app = runtime.block_on(ReloadApp::build(label, size));

            group.throughput(Throughput::Elements(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
                bencher.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;

                    for _ in 0..iterations {
                        app.stage(change);

                        let start = Instant::now();

                        runtime.block_on(app.reload());
                        elapsed += start.elapsed();
                    }

                    elapsed
                });
            });
        }

        group.finish();
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .without_plots()
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));
    targets = reload
}
criterion_main!(benches);
