//! `#[context]` values: seeded in several unrelated scopes, never built.

use super::placements::resolve;
use super::*;
use crate::ScopedConstruction;

/// A context value of type `u128` placed in `scope`.
fn context(id: &'static str, scope: upwell_core::ScopeKey) -> ComponentDescriptor {
    ComponentDescriptor {
        construction: ScopedConstruction::Context,
        ..planned(id, TypeDescriptor::of::<u128>("Identity"), || &[]).with_scope(scope)
    }
}

#[test]
fn a_context_may_be_seeded_in_two_unrelated_scopes() {
    let resolved = resolve(vec![
        context("left", SiblingA::KEY),
        context("right", SiblingB::KEY),
    ])
    .expect("a context is not an ambiguous seed");

    assert_eq!(resolved.len(), 2);
}

#[test]
fn a_context_beside_a_plain_seed_of_its_type_is_still_ambiguous() {
    let plain =
        planned("plain", TypeDescriptor::of::<u128>("Identity"), || &[]).with_scope(SiblingB::KEY);

    let error = resolve(vec![context("left", SiblingA::KEY), plain])
        .expect_err("only contexts share a type across seed scopes");

    assert!(matches!(error, Error::AmbiguousSeedScopes { .. }));
}

#[test]
fn a_context_in_the_root_is_invalid() {
    let registry = ComponentRegistry {
        components: vec![context("root", Singleton::KEY)],
        ..Default::default()
    };

    let error = registry.validate().expect_err("the root opens no instance");

    assert!(matches!(
        error,
        Error::InvalidContext { component_id, .. } if component_id == "root"
    ));
}
