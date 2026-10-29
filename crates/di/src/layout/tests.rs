//! Slot layout planning over marker descriptors.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton,
    Transient, TypeDescriptor,
};

use super::*;
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::error::Error;
use crate::registry::ProviderSelectionModel;
use crate::test_support::{component, dependency, provider, unbuilt_factories};

struct Outer;
struct Inner;
struct Sibling;

impl Scope for Outer {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-outer");
    const RANK: u8 = 2;
    const NAME: &'static str = "Outer";
}

impl Scope for Inner {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-inner");
    const RANK: u8 = 1;
    const NAME: &'static str = "Inner";
}

impl Scope for Sibling {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-sibling");
    const RANK: u8 = 1;
    const NAME: &'static str = "Sibling";
}

trait Marker {}

struct Shared;
struct Connection;
struct Seed;
struct Handler;
struct Fresh;
struct Consumer;
struct Elsewhere;

fn marker() -> TypeDescriptor {
    TypeDescriptor::of::<dyn Marker>("Marker")
}

fn eager<T: 'static>(name: &'static str, cardinality: Cardinality) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(name),
        cardinality,
        ResolutionMode::Eager,
    )
}

fn on_marker(cardinality: Cardinality) -> DependencyDescriptor {
    dependency("Marker", marker(), cardinality, ResolutionMode::Eager)
}

fn scoped(mut descriptor: ComponentDescriptor, scope: ScopeKey) -> ComponentDescriptor {
    descriptor.scope = scope;

    descriptor
}

/// The fixture graph: a root `Shared`, an outer `Connection`, an inner seed and handler,
/// a transient `Fresh`, and an inner `Consumer` of every edge shape.
struct Fixture {
    shared: ComponentDescriptor,
    connection: ComponentDescriptor,
    seed: ComponentDescriptor,
    handler: ComponentDescriptor,
    fresh: ComponentDescriptor,
    consumer: ComponentDescriptor,
    providers: Vec<ProviderDescriptor>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            shared: component::<Shared>("shared", unbuilt_factories!("shared", Vec::new)),
            connection: scoped(
                component::<Connection>(
                    "connection",
                    unbuilt_factories!("connection", || vec![eager::<Shared>(
                        "Shared",
                        Cardinality::One
                    )]),
                ),
                Outer::KEY,
            ),
            seed: scoped(component::<Seed>("seed", || &[]), Inner::KEY),
            handler: scoped(
                component::<Handler>(
                    "handler",
                    unbuilt_factories!("handler", || vec![
                        eager::<Connection>("Connection", Cardinality::One),
                        eager::<Seed>("Seed", Cardinality::One),
                    ]),
                ),
                Inner::KEY,
            ),
            fresh: scoped(
                component::<Fresh>(
                    "fresh",
                    unbuilt_factories!("fresh", || vec![eager::<Shared>(
                        "Shared",
                        Cardinality::One
                    )]),
                ),
                Transient::KEY,
            ),
            consumer: scoped(
                component::<Consumer>(
                    "consumer",
                    unbuilt_factories!("consumer", || vec![
                        eager::<Handler>("Handler", Cardinality::One),
                        eager::<Fresh>("Fresh", Cardinality::One),
                        on_marker(Cardinality::One),
                        on_marker(Cardinality::Collection),
                    ]),
                ),
                Inner::KEY,
            ),
            providers: vec![
                provider::<Handler>(marker(), "handler", true),
                provider::<Shared>(marker(), "shared", false),
            ],
        }
    }

    fn selection(&self, extra: &[ComponentDescriptor]) -> ProviderSelectionModel {
        let mut components = vec![
            self.shared,
            self.connection,
            self.seed,
            self.handler,
            self.fresh,
            self.consumer,
        ];

        components.extend_from_slice(extra);

        ProviderSelectionModel::new(&components, self.providers.clone(), HashMap::new())
            .expect("the fixture providers name fixture components")
    }

    fn outer(&self, selection: &ProviderSelectionModel) -> Arc<SlotLayout> {
        let layout = SlotLayout::plan(
            Outer::KEY,
            &[],
            &[self.connection],
            &HashMap::new(),
            selection,
            reach,
        )
        .expect("the outer scope plans");

        Arc::new(layout)
    }

    fn inner(
        &self,
        selection: &ProviderSelectionModel,
        order: &[ComponentDescriptor],
    ) -> crate::Result<SlotLayout> {
        let ancestors = HashMap::from([(Outer::ID, self.outer(selection))]);

        SlotLayout::plan(
            Inner::KEY,
            &[self.seed],
            order,
            &ancestors,
            selection,
            reach,
        )
    }
}

/// `Inner` opens only under `Outer`, and every scope reaches the root.
fn reach(consumer: ScopeId, dependency: ScopeId) -> bool {
    dependency == Singleton::ID
        || consumer == dependency
        || (consumer == Inner::ID && dependency == Outer::ID)
}

fn sources(layout: &SlotLayout, slot: usize) -> Vec<&DepRef> {
    layout.slots()[slot]
        .dependencies
        .iter()
        .map(|edge| &edge.source)
        .collect()
}

#[test]
fn plan_lays_out_seeds_first_and_indexes_every_slot() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let ids: Vec<&str> = layout.components().map(|component| component.id).collect();

    assert_eq!(
        ids,
        ["seed", "handler", "consumer"],
        "seeds precede factory-backed slots"
    );
    assert!(
        layout.slots()[0].factory.is_none(),
        "a seed slot has no factory"
    );
    assert_eq!(
        layout.slot_of(TypeId::of::<Consumer>()),
        Some(SlotIdx::new(2))
    );
    assert_eq!(
        layout.providers_of(marker().type_id).len(),
        1,
        "only the inner provider of Marker is aliased in the inner scope"
    );
}

#[test]
fn plan_resolves_ancestor_and_local_edges_to_their_slots() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let handler = sources(&layout, 1);

    assert!(
        matches!(handler[0], DepRef::Ancestor { layout, at } if layout.scope().id() == Outer::ID && at.slot == SlotIdx::new(0) && at.alias.is_none()),
        "the outer connection is read from the outer layout's first slot, got {:?}",
        handler[0]
    );
    assert!(
        matches!(handler[1], DepRef::Local(at) if at.slot == SlotIdx::new(0)),
        "the seed is read from the inner seed slot, got {:?}",
        handler[1]
    );
}

#[test]
fn plan_resolves_transient_provider_and_collection_edges() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let consumer = sources(&layout, 2);

    assert!(
        matches!(consumer[0], DepRef::Local(at) if at.slot == SlotIdx::new(1) && at.alias.is_none())
    );
    assert!(
        matches!(consumer[1], DepRef::Transient(plan) if matches!(plan.dependencies[0].source, DepRef::Root(RootTarget::Component(_)))),
        "a transient is planned with its own root edge, got {:?}",
        consumer[1]
    );
    assert!(
        matches!(consumer[2], DepRef::Local(at) if at.slot == SlotIdx::new(1) && at.alias == Some(0)),
        "the nearest Marker provider is the inner handler's first alias, got {:?}",
        consumer[2]
    );

    let DepRef::Many(provided) = consumer[3] else {
        panic!(
            "a collection edge plans every provider, got {:?}",
            consumer[3]
        );
    };
    let qualifiers: Vec<&str> = provided.iter().map(|entry| entry.qualifier).collect();

    assert_eq!(
        qualifiers,
        ["handler", "shared"],
        "providers keep provider order"
    );
    assert!(matches!(
        provided[1].source,
        DepRef::Root(RootTarget::Provider(_))
    ));
}

#[test]
fn plan_leaves_unplanned_edges_to_the_dynamic_path() {
    let fixture = Fixture::new();
    let lazy = scoped(
        component::<Elsewhere>(
            "lazy-consumer",
            unbuilt_factories!("lazy-consumer", || vec![dependency(
                "Handler",
                TypeDescriptor::of::<Handler>("Handler"),
                Cardinality::One,
                ResolutionMode::Lazy,
            )]),
        ),
        Inner::KEY,
    );
    let selection = fixture.selection(&[lazy]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, lazy])
        .expect("the inner scope plans");

    assert!(
        layout.slots()[2].dependencies.is_empty(),
        "lazy edges resolve through their own handle"
    );
}

#[test]
fn plan_marks_an_unsatisfied_optional_edge_absent() {
    let fixture = Fixture::new();
    let optional = scoped(
        component::<Elsewhere>(
            "optional-consumer",
            unbuilt_factories!("optional-consumer", || {
                let mut edge = eager::<Sibling>("Sibling", Cardinality::One);

                edge.optional = true;

                vec![edge]
            }),
        ),
        Inner::KEY,
    );
    let selection = fixture.selection(&[optional]);

    let layout = fixture
        .inner(&selection, &[optional])
        .expect("the inner scope plans");

    assert!(matches!(sources(&layout, 1)[0], DepRef::Absent));
}

#[test]
fn plan_rejects_a_dependency_held_by_no_scope_on_the_chain() {
    let fixture = Fixture::new();
    let elsewhere = scoped(
        component::<Elsewhere>("elsewhere", unbuilt_factories!("elsewhere", Vec::new)),
        Sibling::KEY,
    );
    let consumer = scoped(
        component::<Consumer>(
            "sibling-consumer",
            unbuilt_factories!("sibling-consumer", || vec![eager::<Elsewhere>(
                "Elsewhere",
                Cardinality::One
            )]),
        ),
        Inner::KEY,
    );
    let mut fixture = fixture;

    fixture.consumer = consumer;

    let selection = fixture.selection(&[elsewhere]);
    let error = fixture
        .inner(&selection, &[consumer])
        .expect_err("a sibling scope's component has no slot on the chain");

    assert!(
        matches!(&error, Error::UnplacedDependency { component_id, target, scope, .. } if component_id == "sibling-consumer" && target == "elsewhere" && *scope == "Sibling"),
        "the error names the consumer, target and scope, got {error:?}"
    );
}
