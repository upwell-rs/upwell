//! Builders that stand up DI graphs of a chosen size, layered across several scopes, using only the
//! public `upwell-di` API.
//!
//! Two component families let a bench separate framework overhead from user-data cost:
//!
//! - [`Payloaded<N>`] carries a 64-byte heap payload, so a graph's footprint includes
//!   representative per-component user data.
//! - [`Empty<N>`] is zero-sized, so a graph built from it measures *pure* DI overhead: the
//!   `Arc<ArcSwap<Arc<T>>>` reload cell, the type-erased slot, and the container's index entry —
//!   with no user data at all.
//!
//! Each component is a distinct type — const generics give every `N` its own `TypeId`, which is
//! exactly how the container keys components, so one roster of 128 types yields graphs from a
//! handful of components up to a large multi-scope tree. A graph is `layers` nested scopes
//! (root + children), each holding `width` components; resolving a root component from the deepest
//! scope therefore walks the whole parent chain — the worst case the resolver hits in practice.

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, Scope, ScopeKey, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, ComponentRegistry, Injectable, Live,
    ProviderDescriptor, ScopeContainer, ScopeRegistry,
};

/// The heap payload a [`Payloaded`] component carries.
const PAYLOAD_BYTES: usize = 64;

/// The maximum graph a single roster can express (`width * layers <= 128`).
pub const ROSTER_SIZE: usize = 128;

/// A component carrying a representative 64-byte heap payload.
pub struct Payloaded<const N: usize> {
    _payload: Box<[u8; PAYLOAD_BYTES]>,
}

impl<const N: usize> Default for Payloaded<N> {
    fn default() -> Self {
        Self {
            _payload: Box::new([0u8; PAYLOAD_BYTES]),
        }
    }
}

impl<const N: usize> Component for Payloaded<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "payloaded-component";
    const NAME: &'static str = "PayloadedComponent";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// A zero-sized component: no user data, so its cost in a graph is entirely the container's.
pub struct Empty<const N: usize>;

impl<const N: usize> Default for Empty<N> {
    fn default() -> Self {
        Empty
    }
}

impl<const N: usize> Component for Empty<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "empty-component";
    const NAME: &'static str = "EmptyComponent";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// A trait every bench component provides, so a `Vec<Arc<dyn Svc>>` collection dependency has many
/// providers to gather — the multi-valued resolution path.
pub trait Svc: Send + Sync {}

impl<const N: usize> Svc for Payloaded<N> {}

impl<const N: usize> Svc for Empty<N> {}

/// One roster slot: a component's descriptor, a constructor for a fresh instance, and the provider
/// aliasing it as `dyn Svc`.
#[derive(Clone, Copy)]
pub struct Entry {
    desc: ComponentDescriptor,
    make: fn() -> BoxedComponent,
    provider: ProviderDescriptor,
}

/// Boxes a fresh `T` into the type-erased slot the container seeds.
fn boxed<T>() -> BoxedComponent
where
    T: Component<Handle = Arc<T>> + Default,
{
    BoxedComponent {
        ty: TypeDescriptor::of::<T>(T::NAME),
        value: Box::new(Injectable::into_stored(T::into_handle(T::default()))),
    }
}

/// A manual (factory-less) descriptor for `T`; the instance is seeded directly.
fn desc<T: Component>() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        T::ID,
        T::NAME,
        TypeDescriptor::of::<T>(T::NAME),
        Singleton::KEY,
    )
}

/// Re-erases an already-built `Arc<T>` as `Arc<dyn Svc>` for storage under the trait's key — the
/// same job the `#[component(provide = ..)]` macro generates.
fn erase<T>(boxed: &BoxedComponent) -> BoxedComponent
where
    T: Component<Handle = Arc<T>> + Svc,
{
    let live = boxed
        .value
        .downcast_ref::<Live<T>>()
        .expect("bench component slot has its own stored type");
    let concrete: Arc<T> = live.snapshot();
    let as_trait: Arc<dyn Svc> = concrete;

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn Svc>("dyn Svc"),
        value: Box::new(Injectable::into_stored(as_trait)),
    }
}

/// A provider aliasing `T` under `dyn Svc`.
fn provider<T>() -> ProviderDescriptor
where
    T: Component<Handle = Arc<T>> + Svc,
{
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Svc>("dyn Svc"),
        concrete_ty: TypeDescriptor::of::<T>(T::NAME),
        qualifier: "bench",
        primary: false,
        priority: 0,
        ordering: &[],
        erase: erase::<T>,
        live: upwell_di::provider_live_cell::<dyn Svc>,
    }
}

macro_rules! entries_for {
    ($ctor:ident) => {
        entries_for!(@ $ctor;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67,
            68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
            90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108,
            109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125,
            126, 127)
    };
    (@ $ctor:ident; $($n:literal),* $(,)?) => {
        vec![ $( Entry {
            desc: desc::<$ctor<$n>>(),
            make: boxed::<$ctor<$n>>,
            provider: provider::<$ctor<$n>>(),
        } ),* ]
    };
}

/// A roster of [`ROSTER_SIZE`] distinct [`Payloaded`] component types (64-byte payload each).
pub fn roster() -> Vec<Entry> {
    entries_for!(Payloaded)
}

/// A roster of [`ROSTER_SIZE`] distinct [`Empty`] (zero-sized) component types — for measuring pure
/// DI overhead with no user data.
pub fn empty_roster() -> Vec<Entry> {
    entries_for!(Empty)
}

macro_rules! layer_scopes {
    ($($name:ident = $id:literal : $rank:literal),* $(,)?) => {
        $(
            struct $name;

            impl Scope for $name {
                const ID: upwell_core::ScopeId = upwell_core::namespaced_id!(upwell_core::ScopeId, $id);
                const RANK: u8 = $rank;
                const NAME: &'static str = stringify!($name);
            }
        )*
    };
}

layer_scopes!(
    Layer1 = "benchmark/layer-1": 1,
    Layer2 = "benchmark/layer-2": 2,
    Layer3 = "benchmark/layer-3": 3,
    Layer4 = "benchmark/layer-4": 4,
    Layer5 = "benchmark/layer-5": 5,
    Layer6 = "benchmark/layer-6": 6,
    Layer7 = "benchmark/layer-7": 7,
);

/// Scopes for child layers, indexed by layer number. Index `0` is the root (built as `Singleton`),
/// so it is a placeholder never opened as a child.
static LAYER_SCOPES: [ScopeKey; 8] = [
    Singleton::KEY,
    Layer1::KEY,
    Layer2::KEY,
    Layer3::KEY,
    Layer4::KEY,
    Layer5::KEY,
    Layer6::KEY,
    Layer7::KEY,
];

/// The number of components a graph of `width * layers` holds.
pub fn graph_component_count(width: usize, layers: usize) -> usize {
    width * layers
}

/// Builds a graph of `layers` nested scopes, each seeding `width` distinct components from
/// `entries`. Panics if `width * layers` exceeds the roster.
pub async fn build_graph(entries: &[Entry], width: usize, layers: usize) -> Arc<ScopeContainer> {
    assert!(
        width * layers <= entries.len(),
        "graph needs {} components but roster holds {}",
        width * layers,
        entries.len()
    );
    assert!(layers >= 1 && layers <= LAYER_SCOPES.len(), "1..=8 layers");

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
    .expect("root container builds");

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
        .expect("child scope builds");
    }

    container
}

/// Builds a single root scope seeding `count` components, each registered as a `dyn Svc` provider,
/// so a `Vec<Arc<dyn Svc>>` collection resolves all of them.
///
/// The components are seeded as *instances only* (an empty descriptor slice): `build_root` aliases a
/// provider once when its seed is inserted and again when a matching descriptor is processed, so
/// passing both would register — and later collect — every provider twice. Seeding through the
/// instance path alone registers each provider exactly once.
pub async fn build_with_providers(entries: &[Entry], count: usize) -> Arc<ScopeContainer> {
    let slice = &entries[0..count];
    let components: Vec<ComponentDescriptor> = slice.iter().map(|entry| entry.desc).collect();
    let providers: Vec<ProviderDescriptor> = slice.iter().map(|entry| entry.provider).collect();
    let provider_order = ComponentRegistry {
        components: components.clone(),
        providers: providers.clone(),
    }
    .provider_order(&components)
    .expect("benchmark provider ordering validates");
    let registry = Arc::new(
        ScopeRegistry::new(
            HashMap::new(),
            components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect(),
            providers,
            provider_order,
        )
        .expect("provider scope registry validates"),
    );

    let seeds: Vec<BoxedComponent> = slice.iter().map(|entry| (entry.make)()).collect();

    ScopeContainer::build_root(&[], seeds, ResolverSet::new(), registry)
        .await
        .expect("provider root builds")
}

/// Resolves a root-scoped component through `container` — from the deepest scope this walks the
/// entire parent chain. Returns whether it resolved, for the bench to black-box.
pub fn resolve_root(container: &ScopeContainer) -> bool {
    container.get::<Payloaded<0>>().is_some()
}

/// Extracts a single `Arc<Payloaded<0>>` the way an HTTP/RPC handler parameter is injected.
pub async fn extract_single(container: &Arc<ScopeContainer>) -> bool {
    container.extract::<Arc<Payloaded<0>>>().await.is_ok()
}

/// Extracts an `Option<Arc<Payloaded<0>>>` — the optional-dependency path.
pub async fn extract_optional(container: &Arc<ScopeContainer>) -> bool {
    container
        .extract::<Option<Arc<Payloaded<0>>>>()
        .await
        .is_ok()
}

/// Extracts every `dyn Svc` provider into a `Vec` — the multi-valued collection path.
pub async fn extract_collection(container: &Arc<ScopeContainer>) -> usize {
    container
        .extract::<Vec<Arc<dyn Svc>>>()
        .await
        .map(|all| all.len())
        .unwrap_or(0)
}
