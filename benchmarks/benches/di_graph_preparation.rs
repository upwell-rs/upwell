//! Wall-clock cost of preparing a DI graph: the work startup does once and every reload
//! transition repeats before any component is constructed.
//!
//! - `topological_sort` — wait expansion plus construction ordering of factory-backed singletons.
//! - `effective_graph` — the full `EffectiveGraph::build`: resolved components, provider selection,
//!   validation, demand capture, the reverse index, and the construction order.
//! - `condition_catalog` — building a condition catalog and evaluating it for an empty fact
//!   snapshot, the cost every startup and reload pays even with no conditional component.
//! - `construction` — constructing every factory-backed node in dependency order into a fresh
//!   root, the per-component work a reload pays for each reconstructed fixed dependent.
//! - `reverse_closure` — planning a runtime transition against an unchanged (shared) graph
//!   with one invalidated root: a `leaf` nothing depends on, or the first node, which every
//!   other node depends on transitively. Planning should scale with the closure, not the graph.
//!
//! Sizes double from 256 to 2048 nodes, so linear preparation shows a flat per-element
//! throughput while a quadratic term shows up as throughput halving per step.

use std::collections::{BTreeSet, HashSet};
use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::executor::block_on;
use upwell_benchmarks::graph;
use upwell_core::ResolverSet;
use upwell_core::RuntimeGenerationId;
use upwell_di::{
    ConditionCatalog, ConditionFactSnapshot, EffectiveGraph, ScopeContainer, ScopeRegistry,
    topological_sort,
};

const SIZES: [usize; 4] = [256, 512, 1024, 2048];

fn sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("di_graph_preparation/topological_sort");

    for size in SIZES {
        let registry = graph::registry(size);
        let selection = registry
            .provider_selection_model(&registry.components)
            .expect("graph provider selection builds");
        let prebuilt = HashSet::new();

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
            bencher.iter(|| {
                black_box(
                    topological_sort(
                        black_box(&registry.components),
                        black_box(&prebuilt),
                        black_box(&selection),
                        |_, _| true,
                    )
                    .expect("acyclic graph sorts"),
                )
            });
        });
    }

    group.finish();
}

fn effective(c: &mut Criterion) {
    let mut group = c.benchmark_group("di_graph_preparation/effective_graph");

    for size in SIZES {
        let registry = graph::registry(size);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
            bencher.iter(|| {
                black_box(
                    EffectiveGraph::build(
                        RuntimeGenerationId::INITIAL,
                        black_box(&registry),
                        |_, _| true,
                    )
                    .expect("graph validates"),
                )
            });
        });
    }

    group.finish();
}

fn conditions(c: &mut Criterion) {
    let mut group = c.benchmark_group("di_graph_preparation/condition_catalog");
    let snapshot = ConditionFactSnapshot::new(Vec::new()).expect("empty fact snapshot builds");

    for size in SIZES {
        let registry = graph::registry(size);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
            bencher.iter(|| {
                black_box(
                    ConditionCatalog::new(black_box(&registry), [])
                        .expect("unconditional catalog builds")
                        .evaluate(black_box(&snapshot))
                        .expect("unconditional catalog evaluates"),
                )
            });
        });
    }

    group.finish();
}

fn construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("di_graph_preparation/construction");

    for size in SIZES {
        let registry = graph::registry(size);
        let selection = Arc::new(
            registry
                .provider_selection_model(&registry.components)
                .expect("graph provider selection builds"),
        );
        let order = topological_sort(&registry.components, &HashSet::new(), &selection, |_, _| {
            true
        })
        .expect("acyclic graph sorts")
        .into_iter()
        .copied()
        .collect::<Vec<_>>();
        let scopes = Arc::new(
            ScopeRegistry::from_selection_model(
                Default::default(),
                registry
                    .components
                    .iter()
                    .map(|component| (component.ty.type_id, *component))
                    .collect(),
                selection,
            )
            .expect("graph scope registry builds"),
        );

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
            bencher.iter(|| {
                black_box(
                    block_on(ScopeContainer::build_root(
                        black_box(&order),
                        Vec::new(),
                        ResolverSet::new(),
                        Arc::clone(&scopes),
                    ))
                    .expect("graph root builds"),
                )
            });
        });
    }

    group.finish();
}

fn closure(c: &mut Criterion) {
    let mut group = c.benchmark_group("di_graph_preparation/reverse_closure");

    for size in SIZES {
        let active = EffectiveGraph::build(
            RuntimeGenerationId::INITIAL,
            &graph::registry(size),
            |_, _| true,
        )
        .expect("graph validates");
        let candidate = active.same_graph();
        let roots = [
            ("leaf", graph::components(size)[size - 1].id),
            ("root", graph::components(size)[0].id),
        ];

        group.throughput(Throughput::Elements(size as u64));

        for (label, root) in roots {
            let invalidated = BTreeSet::from([root]);

            group.bench_with_input(BenchmarkId::new(label, size), &size, |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        active
                            .plan_runtime_transition(
                                black_box(&candidate),
                                black_box(&invalidated),
                                |_, _| false,
                            )
                            .expect("candidate shares the active generation"),
                    )
                });
            });
        }
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().without_plots();
    targets = sort, effective, conditions, construction, closure
}
criterion_main!(benches);
