//! DI resolution hot paths — the functions touched on every component injection and every
//! request-time extraction.
//!
//! - `resolver_set_clone`: cloning the request-scope resolver set (an `Arc` bump; the existing
//!   deterministic contract in `upwell-core` proves it is allocation-free — this tracks its
//!   wall-clock cost).
//! - `resolve_by_scope_depth`: `ScopeContainer::get`, resolving a root component from the deepest
//!   scope, across 1/4/8 nested scopes — the parent-chain walk every injected `Arc<T>` performs.
//! - `extract_shapes`: `ScopeContainer::extract`, the `FromContainer` path backing every HTTP/RPC
//!   handler parameter, for a single component, an optional one, and a trait collection.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use upwell_benchmarks::di;
use upwell_core::{Resolver, ResolverSet};

macro_rules! resolver_types {
    ($($name:ident),+ $(,)?) => {
        $(
            struct $name;
            impl Resolver for $name {}
        )+
    };
}

resolver_types!(
    R00, R01, R02, R03, R04, R05, R06, R07, R08, R09, R10, R11, R12, R13, R14, R15, R16, R17, R18,
    R19, R20, R21, R22, R23, R24, R25, R26, R27, R28, R29, R30, R31,
);

fn resolver_sets() -> [(usize, ResolverSet); 3] {
    let mut one = ResolverSet::new();
    one.insert(Arc::new(R00));

    let mut eight = one.clone();
    eight.insert(Arc::new(R01));
    eight.insert(Arc::new(R02));
    eight.insert(Arc::new(R03));
    eight.insert(Arc::new(R04));
    eight.insert(Arc::new(R05));
    eight.insert(Arc::new(R06));
    eight.insert(Arc::new(R07));

    let mut thirty_two = eight.clone();
    thirty_two.insert(Arc::new(R08));
    thirty_two.insert(Arc::new(R09));
    thirty_two.insert(Arc::new(R10));
    thirty_two.insert(Arc::new(R11));
    thirty_two.insert(Arc::new(R12));
    thirty_two.insert(Arc::new(R13));
    thirty_two.insert(Arc::new(R14));
    thirty_two.insert(Arc::new(R15));
    thirty_two.insert(Arc::new(R16));
    thirty_two.insert(Arc::new(R17));
    thirty_two.insert(Arc::new(R18));
    thirty_two.insert(Arc::new(R19));
    thirty_two.insert(Arc::new(R20));
    thirty_two.insert(Arc::new(R21));
    thirty_two.insert(Arc::new(R22));
    thirty_two.insert(Arc::new(R23));
    thirty_two.insert(Arc::new(R24));
    thirty_two.insert(Arc::new(R25));
    thirty_two.insert(Arc::new(R26));
    thirty_two.insert(Arc::new(R27));
    thirty_two.insert(Arc::new(R28));
    thirty_two.insert(Arc::new(R29));
    thirty_two.insert(Arc::new(R30));
    thirty_two.insert(Arc::new(R31));

    [(1, one), (8, eight), (32, thirty_two)]
}

fn resolver_set_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolver_set_clone");

    for (resolvers, set) in resolver_sets() {
        group.bench_with_input(
            BenchmarkId::from_parameter(resolvers),
            &set,
            |bencher, set| bencher.iter(|| black_box(set.clone())),
        );
    }

    group.finish();
}

/// Resolve a root component from the deepest scope, over graphs of increasing scope depth. The
/// width is fixed so only the parent-chain length varies.
fn resolve_by_scope_depth(c: &mut Criterion) {
    const WIDTH: usize = 8;

    let runtime = Runtime::new().expect("tokio runtime");
    let roster = di::roster();

    let mut group = c.benchmark_group("di_resolve_by_scope_depth");

    for layers in [1usize, 4, 8] {
        let container = runtime.block_on(di::build_graph(&roster, WIDTH, layers));

        group.bench_with_input(
            BenchmarkId::from_parameter(layers),
            &container,
            |bencher, container| bencher.iter(|| black_box(di::resolve_root(container))),
        );
    }

    group.finish();
}

/// The three request-time extraction shapes a handler parameter can take.
fn extract_shapes(c: &mut Criterion) {
    const WIDTH: usize = 8;
    const LAYERS: usize = 4;
    const PROVIDERS: usize = 16;

    let runtime = Runtime::new().expect("tokio runtime");
    let roster = di::roster();

    let layered = runtime.block_on(di::build_graph(&roster, WIDTH, LAYERS));
    let with_providers = runtime.block_on(di::build_with_providers(&roster, PROVIDERS));

    // Verify the fixtures actually resolve what the benches assume before timing them: a broken
    // fixture would otherwise silently benchmark a no-op resolution.
    assert!(
        runtime.block_on(di::extract_single(&layered)),
        "single-component fixture does not resolve"
    );
    assert_eq!(
        runtime.block_on(di::extract_collection(&with_providers)),
        PROVIDERS,
        "collection fixture resolved an unexpected provider count"
    );

    let mut group = c.benchmark_group("di_extract");

    group.bench_function("single_arc", |bencher| {
        bencher
            .to_async(&runtime)
            .iter(|| async { black_box(di::extract_single(&layered).await) });
    });

    group.bench_function("optional_arc", |bencher| {
        bencher
            .to_async(&runtime)
            .iter(|| async { black_box(di::extract_optional(&layered).await) });
    });

    group.bench_function(BenchmarkId::new("collection", PROVIDERS), |bencher| {
        bencher
            .to_async(&runtime)
            .iter(|| async { black_box(di::extract_collection(&with_providers).await) });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5))
        .sample_size(60);
    targets = resolver_set_clone, resolve_by_scope_depth, extract_shapes
}
criterion_main!(benches);
