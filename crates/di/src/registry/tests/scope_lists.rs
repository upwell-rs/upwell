//! One component type declaring several scopes, `#[component(scope = [A, B])]` or a factory
//! listing several: one placement per listed scope, and how seeds and overlap interact.

use super::placements::{encloses, factory_ids, resolve};
use super::*;
use crate::test_support::unbuilt_recipe;

use ComponentFactoryRole::{Default, Explicit};

const FALLBACK: ComponentFactoryDescriptor = unbuilt_recipe("fallback", Default, no_dependencies);
const INHERITING: ComponentFactoryDescriptor =
    unbuilt_recipe("inheriting", Explicit, no_dependencies);
const IN_BOTH: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[SiblingA::KEY, SiblingB::KEY],
    ..unbuilt_recipe("in-both", Explicit, no_dependencies)
};
const IN_REQUEST: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[Request::KEY],
    ..unbuilt_recipe("in-request", Explicit, no_dependencies)
};

static DEFAULT_ONLY: [ComponentFactoryDescriptor; 1] = [FALLBACK];
static INHERITING_ONLY: [ComponentFactoryDescriptor; 2] = [FALLBACK, INHERITING];
static LISTED: [ComponentFactoryDescriptor; 1] = [IN_BOTH];
static INHERITING_AND_OTHER: [ComponentFactoryDescriptor; 2] = [INHERITING, IN_REQUEST];

fn default_only() -> &'static [ComponentFactoryDescriptor] {
    &DEFAULT_ONLY
}

fn inheriting_only() -> &'static [ComponentFactoryDescriptor] {
    &INHERITING_ONLY
}

fn listed() -> &'static [ComponentFactoryDescriptor] {
    &LISTED
}

fn inheriting_and_other() -> &'static [ComponentFactoryDescriptor] {
    &INHERITING_AND_OTHER
}

/// The fixture component declared in both sibling scopes, built by `factories`.
fn in_both(factories: fn() -> &'static [ComponentFactoryDescriptor]) -> ComponentDescriptor {
    descriptor_with_factories(factories).with_scopes(&[SiblingA::KEY, SiblingB::KEY])
}

#[test]
fn a_declared_list_places_the_default_factory_in_each_scope() {
    let component = in_both(default_only);
    let resolved = resolve(vec![component]).expect("a declared list resolves");

    assert_eq!(
        component.scope.id(),
        SiblingA::ID,
        "the first scope is declared"
    );
    assert_eq!(
        factory_ids(&resolved),
        [(SiblingA::ID, "fallback"), (SiblingB::ID, "fallback")]
    );
    assert_ne!(
        resolved[0].id, resolved[1].id,
        "each placement has its own id"
    );
    assert!(
        resolved
            .iter()
            .all(|c| c.id.starts_with("factory-fixture@"))
    );
}

#[test]
fn an_unscoped_factory_builds_in_every_declared_scope() {
    let resolved = resolve(vec![in_both(inheriting_only)]).expect("the factory is inherited");

    assert_eq!(
        factory_ids(&resolved),
        [(SiblingA::ID, "inheriting"), (SiblingB::ID, "inheriting")]
    );
}

#[test]
fn a_declared_list_and_a_scoped_factory_combine() {
    let resolved = resolve(vec![in_both(inheriting_and_other)]).expect("three scopes resolve");

    assert_eq!(
        factory_ids(&resolved),
        [
            (SiblingA::ID, "inheriting"),
            (SiblingB::ID, "inheriting"),
            (Request::ID, "in-request")
        ]
    );
}

#[test]
fn a_factory_listing_several_scopes_places_itself_in_each() {
    let component = descriptor_with_factories(listed).with_scope(Request::KEY);
    let resolved = resolve(vec![component]).expect("a factory list resolves");

    assert_eq!(
        factory_ids(&resolved),
        [(SiblingA::ID, "in-both"), (SiblingB::ID, "in-both")],
        "a scoped factory moves the component out of its declared scope"
    );
}

#[test]
fn every_placement_reports_the_declared_list() {
    let resolved = resolve(vec![in_both(default_only)]).expect("a declared list resolves");

    for placement in &resolved {
        assert_eq!(
            placement
                .declared_scopes()
                .iter()
                .map(|scope| scope.id())
                .collect::<Vec<_>>(),
            [SiblingA::ID, SiblingB::ID]
        );
        assert_eq!(placement.placements(), [placement.scope]);
    }
}

#[test]
fn a_seed_replaces_only_the_placement_of_its_own_scope() {
    let built = in_both(default_only);
    let seed = planned("seed", built.ty, || &[]).with_scope(SiblingB::KEY);

    let resolved = resolve(vec![built, seed]).expect("a seed and a factory placement coexist");

    assert_eq!(factory_ids(&resolved[..1]), [(SiblingA::ID, "fallback")]);
    assert_eq!(resolved[1].scope.id(), SiblingB::ID);
    assert!(
        resolved[1]
            .effective_factory()
            .expect("no ambiguity")
            .is_none(),
        "the seed fills the second scope"
    );
}

#[test]
fn a_seed_declared_in_several_scopes_is_rejected() {
    let seed = descriptor_with_factories(|| &[]).with_scopes(&[SiblingA::KEY, SiblingB::KEY]);

    let error = resolve(vec![seed]).expect_err("a seed has one destination");

    assert!(matches!(error, Error::AmbiguousSeedScopes { .. }));
}

#[test]
fn a_declared_list_sharing_a_hierarchy_is_rejected() {
    let component =
        descriptor_with_factories(default_only).with_scopes(&[Request::KEY, Connection::KEY]);
    let resolved = resolve(vec![component]).expect("the list expands");

    let error = ComponentRegistry::default()
        .validate_placement_overlap(&resolved, encloses)
        .expect_err("Connection encloses Request");

    assert!(matches!(
        error,
        Error::OverlappingPlacements {
            outer: "Connection",
            inner: "Request",
            ..
        }
    ));
}

#[test]
fn a_declared_list_including_transient_is_rejected() {
    let component =
        descriptor_with_factories(default_only).with_scopes(&[SiblingA::KEY, Transient::KEY]);

    let error = resolve(vec![component]).expect_err("a transient placement overlaps everything");

    assert!(matches!(error, Error::TransientPlacement { .. }));
}

const TWICE_IN_A: ComponentFactoryDescriptor = ComponentFactoryDescriptor {
    scopes: &[SiblingA::KEY, SiblingA::KEY],
    ..unbuilt_recipe("twice-in-a", Explicit, no_dependencies)
};

static REPEATING: [ComponentFactoryDescriptor; 1] = [TWICE_IN_A];

fn repeating() -> &'static [ComponentFactoryDescriptor] {
    &REPEATING
}

#[test]
fn a_single_scope_set_after_a_list_replaces_the_list() {
    let component = in_both(default_only).with_scope(Request::KEY);
    let resolved = resolve(vec![component]).expect("the single scope resolves");

    assert!(component.scopes.is_empty(), "the list is cleared");
    assert_eq!(component.declared_scopes(), [Request::KEY]);
    assert_eq!(factory_ids(&resolved), [(Request::ID, "fallback")]);
}

#[test]
fn a_list_set_after_a_single_scope_declares_its_first_entry() {
    let component = descriptor_with_factories(default_only)
        .with_scope(Request::KEY)
        .with_scopes(&[SiblingB::KEY, SiblingA::KEY]);

    assert_eq!(component.scope.id(), SiblingB::ID);
}

#[test]
fn a_list_that_does_not_start_with_the_declared_scope_is_rejected() {
    let component = ComponentDescriptor {
        scope: Request::KEY,
        scopes: &[SiblingA::KEY, SiblingB::KEY],
        ..descriptor_with_factories(default_only)
    };

    let error = resolve(vec![component]).expect_err("the scope and its list disagree");

    assert!(
        matches!(
            &error,
            Error::DeclaredScopeMismatch {
                component,
                scope: "Request",
                first: "SiblingA",
            } if component == "FactoryFixture"
        ),
        "{error}"
    );
}

#[test]
fn a_scope_declared_twice_is_rejected() {
    let component =
        descriptor_with_factories(default_only).with_scopes(&[SiblingA::KEY, SiblingA::KEY]);

    let error = resolve(vec![component]).expect_err("one scope listed twice");

    assert!(
        matches!(
            &error,
            Error::RepeatedScope { component, scope: "SiblingA", .. } if component == "FactoryFixture"
        ),
        "{error}"
    );
}

#[test]
fn a_factory_listing_a_scope_twice_is_rejected() {
    let component = descriptor_with_factories(repeating).with_scope(Request::KEY);

    let error = resolve(vec![component]).expect_err("one factory scope listed twice");

    assert!(
        matches!(
            &error,
            Error::RepeatedScope {
                scope: "SiblingA",
                factory: Some("twice-in-a"),
                ..
            }
        ),
        "{error}"
    );
}
