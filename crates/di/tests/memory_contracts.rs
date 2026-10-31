//! Deterministic memory contracts for the DI container hot paths.
//!
//! These are complexity/leak contracts, not wall-clock benchmarks: they assert allocation counts
//! and live-byte deltas, which are independent of runner speed and so run on every pull request
//! (the statistical memory *trend* across graph sizes lives in `benchmarks/di_graph_memory`). The
//! properties proven here:
//!
//! - resolving a component (`get`) allocates nothing — it is an `Arc` bump and a map walk;
//! - request-time extraction (`extract`) leaks nothing — everything it allocates is freed;
//! - a built graph's retained footprint is bounded per component and is fully reclaimed on drop.
//!
//! Allocation tracking is scoped to the measured test thread, so concurrent test-runner activity
//! cannot contaminate the results.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use futures::executor::block_on;
use upwell_core::{ResolverSet, Scope, ScopeId, ScopeKey, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, Injectable, ScopeContainer, ScopeRegistry,
};

// --- allocation tracking -------------------------------------------------------------------------

/// The allocation traffic and net live-byte change across a measured operation.
#[derive(Clone, Copy, Default)]
struct Delta {
    allocations: u64,
    net_live: i64,
}

thread_local! {
    /// Allocator activity in the current thread's active measurement, if any.
    ///
    /// The test harness runs bookkeeping on other threads while the test body executes. Keeping
    /// this state thread-local prevents those unrelated allocations from making the contracts
    /// flaky while still observing every allocation made by the code under measurement.
    static MEASUREMENT: Cell<Option<Delta>> = const { Cell::new(None) };
}

fn record_allocation(allocations: u64, net_live: i64) {
    // Allocation can occur while a thread's TLS is being destroyed, when `with` would panic. Such
    // activity is outside a measurement, so it is safe to ignore an unavailable tracker.
    let _ = MEASUREMENT.try_with(|measurement| {
        if let Some(mut delta) = measurement.get() {
            delta.allocations += allocations;
            delta.net_live += net_live;
            measurement.set(Some(delta));
        }
    });
}

struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(1, layout.size() as i64);

        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_allocation(0, -(layout.size() as i64));

        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(1, layout.size() as i64);

        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation(1, new_size as i64 - layout.size() as i64);

        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn measure<R>(f: impl FnOnce() -> R) -> (R, Delta) {
    MEASUREMENT.with(|measurement| {
        assert!(
            measurement.replace(Some(Delta::default())).is_none(),
            "memory measurements must not be nested"
        );
    });

    let value = f();
    let delta = MEASUREMENT.with(|measurement| {
        measurement
            .replace(None)
            .expect("memory measurement was active")
    });

    (value, delta)
}

#[test]
fn measurement_ignores_allocations_from_other_threads() {
    use std::hint::{black_box, spin_loop};
    use std::sync::atomic::{AtomicBool, Ordering};

    let start = AtomicBool::new(false);
    let allocated = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            while !start.load(Ordering::Acquire) {
                spin_loop();
            }

            let allocation = Box::new([0u8; 64]);
            black_box(&allocation);
            allocated.store(true, Ordering::Release);

            allocation
        });

        let (_, delta) = measure(|| {
            start.store(true, Ordering::Release);
            while !allocated.load(Ordering::Acquire) {
                spin_loop();
            }
        });

        let allocation = worker.join().expect("allocation thread succeeds");
        assert_eq!(allocation.len(), 64, "background allocation was performed");
        assert_eq!(
            delta.allocations, 0,
            "another thread's allocation contaminated the measurement"
        );
        assert_eq!(
            delta.net_live, 0,
            "another thread's live bytes contaminated the measurement"
        );
    });
}

// --- a small graph fixture -----------------------------------------------------------------------

const PAYLOAD_BYTES: usize = 64;

struct C<const N: usize> {
    _payload: Box<[u8; PAYLOAD_BYTES]>,
}

impl<const N: usize> C<N> {
    fn new() -> Self {
        Self {
            _payload: Box::new([0u8; PAYLOAD_BYTES]),
        }
    }
}

impl<const N: usize> Component for C<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "mem-component";
    const NAME: &'static str = "MemComponent";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[derive(Clone, Copy)]
struct Entry {
    desc: ComponentDescriptor,
    make: fn() -> BoxedComponent,
}

fn boxed<const N: usize>() -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<C<N>>(<C<N> as Component>::NAME),
        value: Box::new(Injectable::into_stored(<C<N> as Component>::into_handle(
            C::<N>::new(),
        ))),
    }
}

fn desc<const N: usize>() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        <C<N> as Component>::ID,
        <C<N> as Component>::NAME,
        TypeDescriptor::of::<C<N>>(<C<N> as Component>::NAME),
        Singleton::KEY,
    )
}

macro_rules! roster {
    ($($n:literal),* $(,)?) => {
        vec![ $( Entry { desc: desc::<$n>(), make: boxed::<$n> } ),* ]
    };
}

fn roster() -> Vec<Entry> {
    roster!(
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31
    )
}

struct Layer1;
struct Layer2;
struct Layer3;

impl Scope for Layer1 {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layer-1");
    const RANK: u8 = 1;
    const NAME: &'static str = "Layer1";
}

impl Scope for Layer2 {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layer-2");
    const RANK: u8 = 2;
    const NAME: &'static str = "Layer2";
}

impl Scope for Layer3 {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layer-3");
    const RANK: u8 = 3;
    const NAME: &'static str = "Layer3";
}

static LAYER_SCOPES: [ScopeKey; 4] = [Singleton::KEY, Layer1::KEY, Layer2::KEY, Layer3::KEY];

async fn build_graph(entries: &[Entry], width: usize, layers: usize) -> Arc<ScopeContainer> {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );

    let root = &entries[0..width];
    let root_descs: Vec<ComponentDescriptor> = root.iter().map(|entry| entry.desc).collect();
    let root_seeds: Vec<BoxedComponent> = root.iter().map(|entry| (entry.make)()).collect();

    let mut container = ScopeContainer::build_root(
        &root_descs,
        root_seeds,
        ResolverSet::new(),
        Arc::clone(&registry),
    )
    .await
    .expect("root builds");

    for layer in 1..layers {
        let slice = &entries[layer * width..(layer + 1) * width];
        let descs: Vec<ComponentDescriptor> = slice.iter().map(|entry| entry.desc).collect();
        let seeds: Vec<BoxedComponent> = slice.iter().map(|entry| (entry.make)()).collect();

        container = ScopeContainer::open_child(
            LAYER_SCOPES[layer],
            container,
            Arc::clone(&registry),
            &descs,
            seeds,
        )
        .await
        .expect("child builds");
    }

    container
}

// --- the contracts -------------------------------------------------------------------------------

/// Generous per-component ceiling for a graph's retained footprint: the 64-byte payload plus the
/// `Arc`, the `Live` cell, and the map slot the container keeps for it. Well below any figure that
/// would indicate the container itself is bloating per component.
const PER_COMPONENT_CEILING: i64 = 2048;

#[test]
fn di_memory_contracts() {
    let entries = roster();
    let container = block_on(build_graph(&entries, 8, 4));
    let arc_container = Arc::clone(&container);

    // Warm up the one-time, lazily-initialized thread-locals (arc_swap's debt slot, the futures
    // block_on parker) so the measured regions see only steady-state allocation behaviour. These
    // also assert the fixture resolves — an allocation/leak contract over a failing resolution would
    // pass vacuously.
    assert!(
        container.get::<C<0>>().is_some(),
        "fixture does not resolve C<0>; the allocation contract would be vacuous"
    );
    assert!(
        block_on(async { arc_container.extract::<Arc<C<0>>>().await.is_ok() }),
        "fixture does not extract C<0>; the leak contract would be vacuous"
    );

    // Resolving a component allocates nothing: it is a map walk plus an `Arc` snapshot. The counter
    // also proves every call in the measured region actually resolved.
    let (resolved, resolve_delta) = measure(|| {
        let mut resolved = 0usize;

        for _ in 0..10_000 {
            if container.get::<C<0>>().is_some() {
                resolved += 1;
            }
        }

        resolved
    });

    assert_eq!(
        resolved, 10_000,
        "resolution regressed during the measurement"
    );
    assert_eq!(
        resolve_delta.allocations, 0,
        "ScopeContainer::get allocated on the resolution hot path"
    );

    // Request-time extraction leaks nothing across many calls, and every call succeeds.
    let (extracted, extract_delta) = measure(|| {
        block_on(async {
            let mut extracted = 0usize;

            for _ in 0..10_000 {
                if arc_container.extract::<Arc<C<0>>>().await.is_ok() {
                    extracted += 1;
                }
            }

            extracted
        })
    });

    assert_eq!(
        extracted, 10_000,
        "extraction regressed during the measurement"
    );
    assert_eq!(
        extract_delta.net_live, 0,
        "ScopeContainer::extract leaked {} live bytes over 10k calls",
        extract_delta.net_live
    );

    drop(container);
    drop(arc_container);

    // A built graph's footprint is bounded per component and is fully reclaimed on drop, at every
    // size and scope depth.
    let mut previous_retained = 0;

    for (label, width, layers) in [("small", 2, 1), ("moderate", 4, 2), ("large", 8, 4)] {
        let components = (width * layers) as i64;

        let (graph, build) = measure(|| block_on(build_graph(&entries, width, layers)));
        let retained = build.net_live;

        assert!(
            retained > 0,
            "{label} graph retained no memory — measurement is broken"
        );
        assert!(
            retained / components < PER_COMPONENT_CEILING,
            "{label} graph retained {} bytes/component (ceiling {PER_COMPONENT_CEILING})",
            retained / components
        );
        assert!(
            retained >= previous_retained,
            "{label} graph retained less than a smaller graph — non-monotonic footprint"
        );

        previous_retained = retained;

        let (_, teardown) = measure(|| drop(graph));

        assert_eq!(
            build.net_live + teardown.net_live,
            0,
            "{label} graph leaked {} bytes after drop",
            build.net_live + teardown.net_live
        );
    }
}
