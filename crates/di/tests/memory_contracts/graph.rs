//! The container's footprint and resolution over a layered graph of plain components.

use std::collections::HashMap;
use std::sync::Arc;

use futures::executor::block_on;
use upwell_core::{ResolverSet, Scope, ScopeId, ScopeKey, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, Injectable, ScopeContainer, ScopeRegistry,
};

use crate::measure;

const PAYLOAD_BYTES: usize = 64;

/// A component distinct per `N`, carrying a heap payload so its footprint is measurable.
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

/// A roster component's descriptor and how to build its instance.
#[derive(Clone, Copy)]
struct Entry {
    desc: ComponentDescriptor,
    make: fn() -> BoxedComponent,
}

fn boxed<const N: usize>() -> BoxedComponent {
    BoxedComponent::stored(
        TypeDescriptor::of::<C<N>>(<C<N> as Component>::NAME),
        Injectable::into_stored(<C<N> as Component>::into_handle(C::<N>::new())),
    )
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

/// The first scope layer under the root.
struct Layer1;

/// The second scope layer under the root.
struct Layer2;

/// The third scope layer under the root.
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
        extract_delta.allocations, 0,
        "ScopeContainer::extract allocated reading a built component"
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
