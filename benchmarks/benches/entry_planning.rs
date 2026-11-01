//! Wall-clock cost of preparing a generation as its entry count grows: the candidate graph
//! startup builds once and every reload that changes eligibility builds again, which plans
//! every entry against its scope's layout.
//!
//! - `unique/N` — `N` entries, each with its own target set while the 4096-set pool lasts, so
//!   planning work grows with `N` and large cases spread it across threads.
//! - `shared/N` — `N` entries cycling through 16 target sets, so entries share plans and the
//!   remaining cost is per-entry validation and lookup.
//!
//! Entry counts grow tenfold from 100 to 10 000, so linear preparation shows a flat
//! per-element throughput.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use upwell_app::CandidateGraph;
use upwell_benchmarks::planning::{self, ENTRIES, Signatures};
use upwell_core::RuntimeGenerationId;

fn prepare(c: &mut Criterion) {
    let mut group = c.benchmark_group("entry_planning/prepare");
    let topology = planning::topology();

    for signatures in [Signatures::Unique, Signatures::Shared] {
        for count in ENTRIES {
            let registry = planning::registry(count, signatures);

            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(
                BenchmarkId::new(signatures.label(), count),
                &count,
                |bencher, _| {
                    bencher.iter(|| {
                        black_box(
                            CandidateGraph::prepare(
                                RuntimeGenerationId::INITIAL,
                                black_box(&registry),
                                black_box(&topology),
                            )
                            .expect("the planning registry prepares"),
                        )
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().without_plots();
    targets = prepare
}
criterion_main!(benches);
