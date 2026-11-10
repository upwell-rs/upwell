//! One component type placed in several scopes: expansion, overrides and validation.

use super::*;
use crate::test_support::unbuilt_recipe;

use ComponentFactoryRole::{Default, Explicit};

const IN_SIBLING_A: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[SiblingA::KEY],
    ..unbuilt_recipe("in-a", Explicit, no_dependencies)
};
const IN_SIBLING_B: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[SiblingB::KEY],
    ..unbuilt_recipe("in-b", Explicit, no_dependencies)
};
const AGAIN_IN_SIBLING_A: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[SiblingA::KEY],
    ..unbuilt_recipe("again-in-a", Explicit, no_dependencies)
};
const INHERITING: ComponentFactoryDescriptor =
    unbuilt_recipe("inheriting", Explicit, no_dependencies);
const FALLBACK: ComponentFactoryDescriptor = unbuilt_recipe("fallback", Default, no_dependencies);

static TWO_SCOPES: [ComponentFactoryDescriptor; 2] = [IN_SIBLING_A, IN_SIBLING_B];
static SAME_SCOPE_TWICE: [ComponentFactoryDescriptor; 2] = [IN_SIBLING_A, AGAIN_IN_SIBLING_A];
static DECLARED_AND_OTHER: [ComponentFactoryDescriptor; 2] = [INHERITING, IN_SIBLING_B];
static ONLY_OTHER: [ComponentFactoryDescriptor; 1] = [IN_SIBLING_B];
static DEFAULT_ONLY: [ComponentFactoryDescriptor; 1] = [FALLBACK];

fn two_scopes() -> &'static [ComponentFactoryDescriptor] {
    &TWO_SCOPES
}

fn same_scope_twice() -> &'static [ComponentFactoryDescriptor] {
    &SAME_SCOPE_TWICE
}

fn declared_and_other() -> &'static [ComponentFactoryDescriptor] {
    &DECLARED_AND_OTHER
}

fn only_other() -> &'static [ComponentFactoryDescriptor] {
    &ONLY_OTHER
}

fn default_only() -> &'static [ComponentFactoryDescriptor] {
    &DEFAULT_ONLY
}

fn placed(factories: fn() -> &'static [ComponentFactoryDescriptor]) -> ComponentDescriptor {
    descriptor_with_factories(factories).with_scope(SiblingA::KEY)
}

fn twin(
    id: &'static str,
    scope: upwell_core::ScopeKey,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    planned(id, TypeDescriptor::of::<u128>("Twin"), factories).with_scope(scope)
}

pub(super) fn resolve(
    components: Vec<ComponentDescriptor>,
) -> crate::Result<Vec<ComponentDescriptor>> {
    ComponentRegistry {
        components,
        providers: Vec::new(),
    }
    .resolved_components()
}

pub(super) fn factory_ids(components: &[ComponentDescriptor]) -> Vec<(ScopeId, &'static str)> {
    components
        .iter()
        .map(|component| {
            let factory = component
                .effective_factory()
                .expect("the placement has one factory")
                .expect("the placement is built");

            (component.scope.id(), factory.id)
        })
        .collect()
}

pub(super) fn encloses(outer: ScopeId, inner: ScopeId) -> bool {
    outer == Connection::ID && inner == Request::ID
}

#[test]
fn factories_in_distinct_scopes_expand_to_one_placement_each() {
    let resolved = resolve(vec![placed(two_scopes)]).expect("two scopes resolve");

    assert_eq!(
        factory_ids(&resolved),
        [(SiblingA::ID, "in-a"), (SiblingB::ID, "in-b")]
    );
    assert_ne!(
        resolved[0].id, resolved[1].id,
        "each placement has its own id"
    );
    assert!(resolved[0].id.starts_with("factory-fixture@"));
}

#[test]
fn a_factory_without_a_scope_builds_in_the_declared_scope() {
    let resolved = resolve(vec![placed(declared_and_other)]).expect("two scopes resolve");

    assert_eq!(
        factory_ids(&resolved),
        [(SiblingA::ID, "inheriting"), (SiblingB::ID, "in-b")]
    );
}

#[test]
fn the_only_explicit_factory_moves_the_component_to_its_scope() {
    let resolved = resolve(vec![placed(only_other)]).expect("one scope resolves");

    assert_eq!(factory_ids(&resolved), [(SiblingB::ID, "in-b")]);
    assert_eq!(
        resolved[0].id, "factory-fixture",
        "a lone placement keeps its id"
    );
}

#[test]
fn a_default_factory_stays_in_the_declared_scope() {
    let resolved = resolve(vec![placed(default_only)]).expect("the default resolves");

    assert_eq!(factory_ids(&resolved), [(SiblingA::ID, "fallback")]);
}

#[test]
fn resolving_placements_again_changes_nothing() {
    let first = resolve(vec![placed(two_scopes)]).expect("two scopes resolve");
    let again = resolve(first.clone()).expect("the placements resolve again");

    assert_eq!(
        again
            .iter()
            .map(|c| (c.id, c.scope.id()))
            .collect::<Vec<_>>(),
        first
            .iter()
            .map(|c| (c.id, c.scope.id()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn two_factories_in_one_scope_stay_ambiguous() {
    let error = resolve(vec![placed(same_scope_twice)]).expect_err("one scope, two factories");

    assert!(matches!(error, Error::AmbiguousFactory(_)));
}

#[test]
fn descriptors_of_one_type_in_distinct_scopes_are_kept() {
    let first = twin("first", SiblingA::KEY, default_only);
    let second = twin("second", SiblingB::KEY, default_only);

    let resolved = resolve(vec![first, second]).expect("distinct scopes resolve");

    assert_eq!(resolved.len(), 2);
}

#[test]
fn descriptors_of_one_type_in_one_scope_stay_duplicates() {
    let first = twin("first", SiblingA::KEY, default_only);
    let second = twin("second", SiblingA::KEY, default_only);

    let error = resolve(vec![first, second]).expect_err("one scope, two constructors");

    assert!(matches!(error, Error::DuplicateComponentType(_)));
}

#[test]
fn a_seed_overrides_a_factory_only_within_its_scope() {
    let built = placed(two_scopes);
    let seed = planned("seed", built.ty, || &[]).with_scope(SiblingA::KEY);

    let resolved = resolve(vec![built, seed]).expect("the seed replaces one placement");

    assert_eq!(resolved.len(), 2);
    assert!(
        resolved[0]
            .effective_factory()
            .expect("no ambiguity")
            .is_none(),
        "the seed took the first placement's position"
    );
    assert_eq!(resolved[1].scope.id(), SiblingB::ID);
}

#[test]
fn a_seed_in_two_scopes_is_rejected() {
    let ty = TypeDescriptor::of::<u128>("Twin");
    let first = planned("first", ty, || &[]).with_scope(SiblingA::KEY);
    let second = planned("second", ty, || &[]).with_scope(SiblingB::KEY);

    let error = resolve(vec![first, second]).expect_err("a seed has one scope");

    assert!(matches!(error, Error::AmbiguousSeedScopes { .. }));
}

#[test]
fn a_transient_placement_beside_another_is_rejected() {
    let ty = TypeDescriptor::of::<u128>("Twin");
    let scoped = planned("first", ty, default_only).with_scope(SiblingA::KEY);
    let transient = planned("second", ty, default_only).with_scope(Transient::KEY);

    let error = resolve(vec![scoped, transient]).expect_err("a transient is everywhere");

    assert!(matches!(error, Error::TransientPlacement { .. }));
}

#[test]
fn placements_that_share_a_hierarchy_are_rejected() {
    let ty = TypeDescriptor::of::<u128>("Twin");
    let outer = planned("outer", ty, default_only).with_scope(Connection::KEY);
    let inner = planned("inner", ty, default_only).with_scope(Request::KEY);
    let registry = ComponentRegistry::default();

    let error = registry
        .validate_placement_overlap(&[inner, outer], encloses)
        .expect_err("Connection encloses Request");

    assert!(matches!(
        error,
        Error::OverlappingPlacements {
            outer: "Connection",
            inner: "Request",
            ..
        }
    ));
    assert!(error.to_string().contains("may not share a hierarchy"));
}

#[test]
fn disjoint_placements_are_accepted() {
    let resolved = resolve(vec![placed(two_scopes)]).expect("two scopes resolve");

    ComponentRegistry::default()
        .validate_placement_overlap(&resolved, encloses)
        .expect("siblings do not share a hierarchy");
}

#[test]
fn a_consumer_reaches_the_placement_in_its_own_scope() {
    static NEEDS_TWIN: [DependencyDescriptor; 1] = [dependency(
        "Twin",
        TypeDescriptor::of::<u128>("Twin"),
        Cardinality::One,
        ResolutionMode::Eager,
    )];

    let twin = placed(two_scopes);
    let twin = ComponentDescriptor {
        ty: TypeDescriptor::of::<u128>("Twin"),
        ..twin
    };
    let in_a = scoped!(
        "InA",
        SiblingA::KEY,
        &NEEDS_TWIN,
        TypeDescriptor::of::<u8>("InA")
    );
    let in_b = scoped!(
        "InB",
        SiblingB::KEY,
        &NEEDS_TWIN,
        TypeDescriptor::of::<u16>("InB")
    );
    let elsewhere = scoped!(
        "Elsewhere",
        Request::KEY,
        &NEEDS_TWIN,
        TypeDescriptor::of::<u32>("Elsewhere")
    );
    let reach = |consumer: ScopeId, dependency: ScopeId| consumer == dependency;

    let reached = ComponentRegistry {
        components: vec![twin, in_a, in_b],
        providers: Vec::new(),
    }
    .validate_with_scope_reachability(reach, |_, _| false);
    let missed = ComponentRegistry {
        components: vec![twin, elsewhere],
        providers: Vec::new(),
    }
    .validate_with_scope_reachability(reach, |_, _| false);

    reached.expect("each consumer reaches the placement of its own scope");
    assert!(matches!(missed, Err(Error::ScopeViolation(_))));
}
