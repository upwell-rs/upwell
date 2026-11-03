//! One component type built in several scopes: each scope plans its own slot.

use upwell_di::ComponentFactoryRole;

use super::*;

/// A component with one factory per scope.
struct Twin;
/// A child consumer of [`Twin`].
struct ChildUser;
/// A sibling consumer of [`Twin`].
struct SiblingUser;

fn twin_edge() -> Vec<DependencyDescriptor> {
    vec![one::<Twin>("twin", "Twin")]
}

/// The never-driven factory of [`Twin`] building in `scope`.
const fn twin_in(id: &'static str, scope: ScopeKey) -> ComponentFactoryDescriptor {
    ComponentFactoryDescriptor {
        id,
        construct: crate::test_support::unbuilt,
        construct_ready: None,
        dependencies: Vec::new,
        role: ComponentFactoryRole::Explicit,
        scope: Some(scope),
    }
}

fn twin_factories() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[
            twin_in("twin-in-child", Child::KEY),
            twin_in("twin-in-sibling", Sibling::KEY),
        ]
    }
}

fn resolved(extra: Vec<ComponentDescriptor>) -> Vec<ComponentDescriptor> {
    let mut components = vec![descriptor::<Twin>(
        "twin",
        "Twin",
        Singleton::KEY,
        twin_factories,
    )];

    components.extend(extra);

    ComponentRegistry {
        components,
        providers: Vec::new(),
    }
    .resolved_components()
    .expect("the placements resolve")
}

fn consumers() -> Vec<ComponentDescriptor> {
    vec![
        descriptor::<ChildUser>("child-user", "ChildUser", Child::KEY, || {
            unbuilt_factories!("child-user", twin_edge)()
        }),
        descriptor::<SiblingUser>("sibling-user", "SiblingUser", Sibling::KEY, || {
            unbuilt_factories!("sibling-user", twin_edge)()
        }),
    ]
}

fn plan(components: &[ComponentDescriptor]) -> ScopePlan {
    ScopePlan::partition(
        components,
        &selection(components),
        &topology(),
        &[],
        UnplannedConstruction::Allow,
    )
    .expect("placements plan")
}

#[test]
fn each_scope_plans_its_own_slot_for_the_type() {
    let components = resolved(consumers());
    let plan = plan(&components);

    for scope in [CHILD_ID, SIBLING_ID] {
        let layout = plan.layouts.get(&scope).expect("the scope has a layout");
        let twins = layout
            .components()
            .filter(|component| component.ty.type_id == TypeId::of::<Twin>())
            .count();

        assert_eq!(twins, 1, "one Twin slot in {scope}");
    }

    assert!(plan.layouts.get(&PARENT_ID).is_none_or(|layout| {
        layout
            .components()
            .all(|component| component.ty.type_id != TypeId::of::<Twin>())
    }));
}

#[test]
fn consumers_plan_against_the_slot_of_their_own_scope() {
    let components = resolved(consumers());
    let plan = plan(&components);

    for (scope, user) in [
        (CHILD_ID, TypeId::of::<ChildUser>()),
        (SIBLING_ID, TypeId::of::<SiblingUser>()),
    ] {
        let layout = plan.layouts.get(&scope).expect("the scope has a layout");

        assert!(
            layout
                .components()
                .any(|component| component.ty.type_id == user),
            "{scope} builds its consumer"
        );
    }
}

#[test]
fn a_placement_inside_another_placements_hierarchy_is_rejected() {
    let mut components = resolved(Vec::new());
    let parent = ComponentDescriptor {
        scope: Parent::KEY,
        id: "twin-in-parent",
        ..components[0]
    };

    components.push(parent);

    let registry = ComponentRegistry::default();
    let topology = topology();
    let error = registry
        .validate_placement_overlap(&components, |outer, inner| {
            topology.encloses(&outer, &inner)
        })
        .expect_err("Parent encloses Child");

    assert!(error.to_string().contains("may not share a hierarchy"));
}

#[test]
fn enclosure_follows_any_opening_chain() {
    let topology = topology();

    assert!(topology.encloses(&PARENT_ID, &CHILD_ID));
    assert!(topology.encloses(&Singleton::ID, &CHILD_ID));
    assert!(!topology.encloses(&CHILD_ID, &PARENT_ID));
    assert!(!topology.encloses(&SIBLING_ID, &CHILD_ID));
    assert!(!topology.encloses(&CHILD_ID, &Singleton::ID));
}
