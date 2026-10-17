use std::collections::HashSet;

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode};

use super::*;
use crate::test_support::{component, unbuilt_factories};
use crate::{ComponentFactoryDescriptor, ComponentRegistry};

fn dependency<T: 'static>(resolution: ResolutionMode) -> DependencyDescriptor {
    let name = std::any::type_name::<T>();

    crate::test_support::dependency(
        name,
        upwell_core::TypeDescriptor::of::<T>(name),
        Cardinality::One,
        resolution,
    )
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

fn a_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", a_dependencies, true)()
}

fn b_eager_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", b_eager_dependencies, true)()
}

fn b_deferred_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", b_deferred_dependencies, true)()
}

fn c_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", c_dependencies, true)()
}

fn c_to_d_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", c_to_d_dependencies, true)()
}

fn d_to_c_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", d_to_c_dependencies, true)()
}

fn e_to_a_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", e_to_a_dependencies, true)()
}

fn a_to_b_and_c_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", a_to_b_and_c_dependencies, true)()
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
        component::<A>("a", a_factory),
        component::<B>("b", b_deferred_factory),
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
        component::<C>("c", c_factory),
        component::<B>("b", b_eager_factory),
        component::<A>("a", a_factory),
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
        component::<E>("e", e_to_a_factory),
        component::<D>("d", d_to_c_factory),
        component::<B>("b", b_eager_factory),
        component::<C>("c", c_to_d_factory),
        component::<A>("a", a_factory),
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
        component::<D>("d", d_to_c_factory),
        component::<B>("b", b_eager_factory),
        component::<C>("c", c_to_d_factory),
        component::<A>("a", a_to_b_and_c_factory),
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
