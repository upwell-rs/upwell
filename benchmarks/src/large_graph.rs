//! Registries with many components and many entries, for measuring how preparing a generation
//! scales when an application's graph is large.
//!
//! Components spread over the root and two nested scopes, and each reads up to three earlier
//! components it can reach, so sorting and layout planning have real edges to walk. Each entry
//! injects one component of the innermost scope, so a registry has at most that many distinct
//! target sets. Factories are never run: preparing a generation only reads their descriptors.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use upwell_app::{
    AppRegistry, EntryDescriptor, PreparedScopeTopology, ScopeBoundary, ScopeParent, ScopeTopology,
};
use upwell_core::{
    DependencyDescriptor, EntryKey, Scope, ScopeId, ScopeKey, Singleton, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    ComponentFactoryRole, FromContainer,
};

/// The number of components the fixture can hold, and the largest count it measures.
pub const MAX_COMPONENTS: usize = 10_000;

/// The component counts measured.
pub const COMPONENTS: [usize; 4] = [200, 2_000, 5_000, MAX_COMPONENTS];

/// The entry counts measured for each component count.
pub const ENTRIES: [usize; 2] = [1_000, 10_000];

/// A scope under the root.
pub struct Outer;

/// A scope under [`Outer`], which every entry opens.
pub struct Inner;

impl Scope for Outer {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "bench/large-outer");
    const RANK: u8 = 100;
    const NAME: &'static str = "Outer";
}

impl Scope for Inner {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "bench/large-inner");
    const RANK: u8 = 50;
    const NAME: &'static str = "Inner";
}

static BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new::<Outer>(&[ScopeParent::Root]),
    ScopeBoundary::new::<Inner>(&[ScopeParent::of::<Outer>()]),
];

/// One component type, told apart by the four decimal digits of its index.
struct Node<const A: usize, const B: usize, const C: usize, const D: usize>;

/// The prepared topology of [`Outer`] and [`Inner`].
pub fn topology() -> PreparedScopeTopology {
    ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect("the large-graph topology validates")
}

/// How deeply a component's scope nests, in nesting order.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Depth {
    Root,
    Outer,
    Inner,
}

impl Depth {
    /// The depth of the scope of component `index`.
    const fn of(index: usize) -> Self {
        match index % 10 {
            0..=3 => Self::Root,
            4..=6 => Self::Outer,
            _ => Self::Inner,
        }
    }

    fn scope(self) -> ScopeKey {
        match self {
            Self::Root => Singleton::KEY,
            Self::Outer => Outer::KEY,
            Self::Inner => Inner::KEY,
        }
    }
}

/// The type of every component, by index. Built once before any dependency edge is read.
fn types() -> &'static [TypeDescriptor] {
    static TYPES: OnceLock<Vec<TypeDescriptor>> = OnceLock::new();

    TYPES.get_or_init(|| roster().iter().map(|node| node.ty).collect())
}

/// The earlier components `index` reads: those of `index / 2`, `index / 3` and `index - 1` its
/// scope can reach, so the edges always form a DAG.
fn parents(index: usize) -> impl Iterator<Item = usize> {
    let mut parents = [index / 2, index / 3, index.saturating_sub(1)];
    let mut previous = None;

    parents.sort_unstable();

    parents.into_iter().filter(move |parent| {
        let keep =
            *parent != index && previous != Some(*parent) && Depth::of(*parent) <= Depth::of(index);

        previous = Some(*parent);

        keep
    })
}

fn dependencies<const A: usize, const B: usize, const C: usize, const D: usize>()
-> Vec<DependencyDescriptor> {
    let index = A * 1000 + B * 100 + C * 10 + D;
    let types = types();

    parents(index)
        .map(|parent| DependencyDescriptor {
            ty: types[parent],
            ..<Arc<Node<A, B, C, D>> as FromContainer>::dependency()
        })
        .collect()
}

fn construct(
    _cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    unreachable!("preparing a generation never runs a factory")
}

fn factories<const A: usize, const B: usize, const C: usize, const D: usize>()
-> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct,
            construct_ready: None,
            dependencies: dependencies::<A, B, C, D>,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn node<const A: usize, const B: usize, const C: usize, const D: usize>() -> ComponentDescriptor {
    let index = A * 1000 + B * 100 + C * 10 + D;
    let id: &'static str = Box::leak(format!("large-graph-{index}").into_boxed_str());

    ComponentDescriptor {
        factories: factories::<A, B, C, D>,
        ..ComponentDescriptor::manual(
            id,
            id,
            TypeDescriptor::of::<Node<A, B, C, D>>(id),
            Depth::of(index).scope(),
        )
    }
}

fn targets<const A: usize, const B: usize, const C: usize, const D: usize>()
-> Vec<DependencyDescriptor> {
    vec![<Arc<Node<A, B, C, D>> as FromContainer>::dependency()]
}

/// One component and the targets of an entry injecting it.
type Member = (ComponentDescriptor, fn() -> Vec<DependencyDescriptor>);

fn member<const A: usize, const B: usize, const C: usize, const D: usize>() -> Member {
    (node::<A, B, C, D>(), targets::<A, B, C, D>)
}

fn ones<const A: usize, const B: usize, const C: usize>(members: &mut Vec<Member>) {
    members.push(member::<A, B, C, 0>());
    members.push(member::<A, B, C, 1>());
    members.push(member::<A, B, C, 2>());
    members.push(member::<A, B, C, 3>());
    members.push(member::<A, B, C, 4>());
    members.push(member::<A, B, C, 5>());
    members.push(member::<A, B, C, 6>());
    members.push(member::<A, B, C, 7>());
    members.push(member::<A, B, C, 8>());
    members.push(member::<A, B, C, 9>());
}

fn tens<const A: usize, const B: usize>(members: &mut Vec<Member>) {
    ones::<A, B, 0>(members);
    ones::<A, B, 1>(members);
    ones::<A, B, 2>(members);
    ones::<A, B, 3>(members);
    ones::<A, B, 4>(members);
    ones::<A, B, 5>(members);
    ones::<A, B, 6>(members);
    ones::<A, B, 7>(members);
    ones::<A, B, 8>(members);
    ones::<A, B, 9>(members);
}

fn hundreds<const A: usize>(members: &mut Vec<Member>) {
    tens::<A, 0>(members);
    tens::<A, 1>(members);
    tens::<A, 2>(members);
    tens::<A, 3>(members);
    tens::<A, 4>(members);
    tens::<A, 5>(members);
    tens::<A, 6>(members);
    tens::<A, 7>(members);
    tens::<A, 8>(members);
    tens::<A, 9>(members);
}

fn members() -> &'static [Member] {
    static MEMBERS: OnceLock<Vec<Member>> = OnceLock::new();

    MEMBERS.get_or_init(|| {
        let mut members = Vec::with_capacity(MAX_COMPONENTS);

        hundreds::<0>(&mut members);
        hundreds::<1>(&mut members);
        hundreds::<2>(&mut members);
        hundreds::<3>(&mut members);
        hundreds::<4>(&mut members);
        hundreds::<5>(&mut members);
        hundreds::<6>(&mut members);
        hundreds::<7>(&mut members);
        hundreds::<8>(&mut members);
        hundreds::<9>(&mut members);

        members
    })
}

fn roster() -> Vec<ComponentDescriptor> {
    members().iter().map(|(node, _)| *node).collect()
}

/// A registry of the first `components` components and `entries` entries into [`Inner`], each
/// injecting one of its components in turn. Entry keys are leaked, as the registry borrows them
/// for the program's lifetime.
pub fn registry(components: usize, entries: usize) -> AppRegistry {
    let held = &members()[..components];
    let injected: Vec<_> = held
        .iter()
        .enumerate()
        .filter(|(index, _)| Depth::of(*index) == Depth::Inner)
        .map(|(_, (_, targets))| *targets)
        .collect();
    let entries = (0..entries)
        .map(|index| {
            let key = Box::leak(format!("bench/large-{index}").into_boxed_str());

            EntryDescriptor {
                key: EntryKey::new(key).expect("bench entry keys are valid"),
                scope: Inner::KEY,
                targets: injected[index % injected.len()],
                owner: None,
                source: None,
            }
        })
        .collect();

    AppRegistry {
        components: held.iter().map(|(node, _)| *node).collect(),
        entries,
        ..AppRegistry::default()
    }
}
