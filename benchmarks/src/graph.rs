//! Large factory-backed singleton graphs for measuring graph *preparation*: wait expansion,
//! topological ordering, and effective-graph construction, which run at startup and again on
//! every reload transition.
//!
//! Every node is a distinct const-generic type (`Node<HI, LO>`), so each has its own `TypeId`,
//! and declares an explicit factory with up to three required eager dependencies on earlier
//! nodes (`i - 1`, `i / 2`, `i / 3`). The graph is acyclic, every node waits on real edges, and
//! its depth grows logarithmically-to-linearly with size, the shape a generated application
//! with a few thousand components produces.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, Singleton,
    TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, ComponentRegistry, Injectable,
};

/// Nodes per row of the const-generic roster (`LO` ranges over `0..ROW`).
const ROW: usize = 32;

/// The largest graph the roster can express.
pub const MAX_NODES: usize = ROW * 64;

/// The type descriptors of every roster node, indexed by node number, read by the
/// dependency functions to name a node's predecessors.
static NODES: OnceLock<Vec<TypeDescriptor>> = OnceLock::new();

/// A zero-sized singleton whose position in the roster is `HI * ROW + LO`.
pub struct Node<const HI: usize, const LO: usize>;

impl<const HI: usize, const LO: usize> Component for Node<HI, LO> {
    type Handle = Arc<Self>;

    const ID: &'static str = "graph-node";
    const NAME: &'static str = "GraphNode";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

fn construct<const HI: usize, const LO: usize>(
    _cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<Node<HI, LO>>(Node::<HI, LO>::NAME),
            value: Box::new(Injectable::into_stored(Node::<HI, LO>.into_handle())),
        })
    })
}

fn dependencies<const HI: usize, const LO: usize>() -> Vec<DependencyDescriptor> {
    let index = HI * ROW + LO;
    let nodes = NODES.get().expect("graph roster is initialised before use");
    let mut predecessors = [index.wrapping_sub(1), index / 2, index / 3]
        .into_iter()
        .filter(|predecessor| *predecessor < index)
        .collect::<Vec<_>>();

    predecessors.dedup();

    predecessors
        .into_iter()
        .map(|predecessor| DependencyDescriptor {
            name: "predecessor",
            ty: nodes[predecessor],
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Eager,
            observation: DependencyObservation::Snapshot,
        })
        .collect()
}

fn factories<const HI: usize, const LO: usize>() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct: construct::<HI, LO>,
            dependencies: dependencies::<HI, LO>,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn node<const HI: usize, const LO: usize>() -> ComponentDescriptor {
    let index = HI * ROW + LO;

    ComponentDescriptor {
        id: Box::leak(format!("graph-node-{index}").into_boxed_str()),
        name: Node::<HI, LO>::NAME,
        ty: TypeDescriptor::of::<Node<HI, LO>>(Node::<HI, LO>::NAME),
        scope: &Singleton,
        condition: None,
        factories: factories::<HI, LO>,
        hooks: || &[],
        generation_snapshot: None,
    }
}

macro_rules! row {
    ($nodes:ident, $hi:literal) => {
        row!(@ $nodes, $hi;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31)
    };
    (@ $nodes:ident, $hi:literal; $($lo:literal),*) => {
        $( $nodes.push(node::<$hi, $lo>()); )*
    };
}

macro_rules! rows {
    ($nodes:ident; $($hi:literal),*) => {
        $( row!($nodes, $hi); )*
    };
}

/// Every roster node in index order. Initialises the dependency lookup table on first use.
fn roster() -> &'static [ComponentDescriptor] {
    static ROSTER: OnceLock<Vec<ComponentDescriptor>> = OnceLock::new();

    ROSTER.get_or_init(|| {
        let mut nodes = Vec::with_capacity(MAX_NODES);

        rows!(nodes;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63);

        NODES
            .set(nodes.iter().map(|node| node.ty).collect())
            .expect("graph roster initialises once");

        nodes
    })
}

/// A registry of the first `count` roster nodes, with no providers. Panics if `count`
/// exceeds [`MAX_NODES`].
pub fn registry(count: usize) -> ComponentRegistry {
    assert!(
        count <= MAX_NODES,
        "graph needs {count} nodes but the roster holds {MAX_NODES}"
    );

    ComponentRegistry {
        components: roster()[..count].to_vec(),
        providers: Vec::new(),
    }
}
