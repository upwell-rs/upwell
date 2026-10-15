use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use upwell_core::{Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode};

use super::*;
use crate::{
    BoxedComponent, ComponentConstructionContext, ComponentFactoryDescriptor, ComponentRegistry,
};

fn panic_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    panic!("observability tests do not construct components")
}

fn dependency<T: 'static>(resolution: ResolutionMode) -> DependencyDescriptor {
    DependencyDescriptor {
        name: std::any::type_name::<T>(),
        ty: upwell_core::TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution,
        observation: DependencyObservation::Snapshot,
    }
}

fn descriptor<T: 'static>(
    id: &'static str,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name: id,
        ty: upwell_core::TypeDescriptor::of::<T>(id),
        scope: &Singleton,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

struct A;
struct B;
struct C;
struct D;
struct E;

fn a_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<B>(ResolutionMode::Eager)]
}

fn b_eager_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<A>(ResolutionMode::Eager)]
}

fn b_deferred_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<A>(ResolutionMode::Deferred)]
}

fn c_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<A>(ResolutionMode::Eager)]
}

fn c_to_d_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<D>(ResolutionMode::Eager)]
}

fn d_to_c_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<C>(ResolutionMode::Eager)]
}

fn e_to_a_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<A>(ResolutionMode::Eager)]
}

fn a_to_b_and_c_dependencies() -> Vec<DependencyDescriptor> {
    vec![
        dependency::<B>(ResolutionMode::Eager),
        dependency::<C>(ResolutionMode::Eager),
    ]
}

static A_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: a_dependencies,
    default: true,
}];
static B_EAGER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: b_eager_dependencies,
    default: true,
}];
static B_DEFERRED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: b_deferred_dependencies,
    default: true,
}];
static C_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: c_dependencies,
    default: true,
}];
static C_TO_D_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: c_to_d_dependencies,
    default: true,
}];
static D_TO_C_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: d_to_c_dependencies,
    default: true,
}];
static E_TO_A_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: e_to_a_dependencies,
    default: true,
}];
static A_TO_B_AND_C_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: panic_factory,
    dependencies: a_to_b_and_c_dependencies,
    default: true,
}];

fn a_factory() -> &'static [ComponentFactoryDescriptor] {
    &A_FACTORY
}

fn b_eager_factory() -> &'static [ComponentFactoryDescriptor] {
    &B_EAGER_FACTORY
}

fn b_deferred_factory() -> &'static [ComponentFactoryDescriptor] {
    &B_DEFERRED_FACTORY
}

fn c_factory() -> &'static [ComponentFactoryDescriptor] {
    &C_FACTORY
}

fn c_to_d_factory() -> &'static [ComponentFactoryDescriptor] {
    &C_TO_D_FACTORY
}

fn d_to_c_factory() -> &'static [ComponentFactoryDescriptor] {
    &D_TO_C_FACTORY
}

fn e_to_a_factory() -> &'static [ComponentFactoryDescriptor] {
    &E_TO_A_FACTORY
}

fn a_to_b_and_c_factory() -> &'static [ComponentFactoryDescriptor] {
    &A_TO_B_AND_C_FACTORY
}

fn selection(components: &[ComponentDescriptor]) -> ProviderSelectionModel {
    ComponentRegistry {
        components: components.to_vec(),
        providers: Vec::new(),
    }
    .provider_selection_model(components)
    .expect("selection model validates")
}

#[test]
fn deferred_edges_are_reported_as_cycle_breaks() {
    let components = [
        descriptor::<A>("a", a_factory),
        descriptor::<B>("b", b_deferred_factory),
    ];
    let selection = selection(&components);

    let events = crate::test_support::capture_events(|| {
        topological_sort(&components, &HashSet::new(), &selection, |_, _| true)
            .expect("deferred edge breaks construction cycle");
    });

    assert!(events.iter().any(|event| {
        event.target == crate::observability::GRAPH_TARGET
            && event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "graph-edge-declaration")
            && event
                .fields
                .get("reason")
                .is_some_and(|value| value == "deferred-cycle-break")
            && event
                .fields
                .get("accepted")
                .is_some_and(|value| value == "false")
    }));
}

#[test]
fn eager_cycles_distinguish_members_from_blocked_components() {
    let components = [
        descriptor::<C>("c", c_factory),
        descriptor::<B>("b", b_eager_factory),
        descriptor::<A>("a", a_factory),
    ];
    let selection = selection(&components);

    let events = crate::test_support::capture_events(|| {
        let _ = topological_sort(&components, &HashSet::new(), &selection, |_, _| true)
            .expect_err("eager cycle rejects construction order");
    });
    let members = events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "cycle-member")
        })
        .filter_map(|event| event.fields.get("component_id").cloned())
        .collect::<Vec<_>>();
    let blocked = events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "cycle-blocked")
        })
        .filter_map(|event| event.fields.get("component_id").cloned())
        .collect::<Vec<_>>();

    assert_eq!(members, ["a", "b"]);
    assert_eq!(blocked, ["c"]);
}

#[test]
fn independent_eager_cycles_have_separate_cycle_identities() {
    let components = [
        descriptor::<E>("e", e_to_a_factory),
        descriptor::<D>("d", d_to_c_factory),
        descriptor::<B>("b", b_eager_factory),
        descriptor::<C>("c", c_to_d_factory),
        descriptor::<A>("a", a_factory),
    ];
    let selection = selection(&components);

    let events = crate::test_support::capture_events(|| {
        let _ = topological_sort(&components, &HashSet::new(), &selection, |_, _| true)
            .expect_err("independent eager cycles reject construction order");
    });
    let summaries = events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "cycle-summary")
        })
        .filter_map(|event| event.fields.get("cycle_id").cloned())
        .collect::<Vec<_>>();

    assert_eq!(summaries, ["a", "c"]);

    let blocked = events
        .iter()
        .filter(|event| {
            event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "cycle-blocked")
        })
        .map(|event| {
            (
                event.fields["cycle_id"].clone(),
                event.fields["component_id"].clone(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(blocked, [("a".to_string(), "e".to_string())]);
}

#[test]
fn members_of_one_cycle_are_not_blocked_members_of_another() {
    let components = [
        descriptor::<D>("d", d_to_c_factory),
        descriptor::<B>("b", b_eager_factory),
        descriptor::<C>("c", c_to_d_factory),
        descriptor::<A>("a", a_to_b_and_c_factory),
    ];
    let selection = selection(&components);

    let events = crate::test_support::capture_events(|| {
        let _ = topological_sort(&components, &HashSet::new(), &selection, |_, _| true)
            .expect_err("linked eager cycles reject construction order");
    });

    assert!(!events.iter().any(|event| {
        event
            .fields
            .get("event_name")
            .is_some_and(|value| value == "cycle-blocked")
    }));
}
