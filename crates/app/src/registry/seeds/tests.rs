use upwell_core::{Scope, TypeDescriptor};
use upwell_di::ComponentDescriptor;

use super::*;

/// A claimed value.
struct Identity;

/// A scope the claims name.
struct Session;

impl Scope for Session {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/seed-claims-session");
    const RANK: u8 = 1;
    const NAME: &'static str = "Session";
}

/// The factory-less slot of [`Identity`] in [`Session`].
fn slot() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        "identity",
        "Identity",
        TypeDescriptor::of::<Identity>("Identity"),
        Session::KEY,
    )
}

#[test]
fn one_claimant_claims_a_seed_once() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Session>("endpoint"));
    registry.claim_seed(SeedClaim::of::<Identity, Session>("endpoint"));

    assert_eq!(registry.seed_claims.len(), 1);
}

#[test]
fn two_openers_may_claim_one_seed_for_their_own_instances() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Session>("first protocol"));
    registry.claim_seed(SeedClaim::of::<Identity, Session>("second protocol"));

    let claimed = registry
        .claimed_scopes(&[slot()])
        .expect("each opener fills its own instances");

    assert_eq!(claimed.len(), 1);
}

#[test]
fn a_claim_for_a_scope_without_a_slot_is_rejected() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Session>("seeder"));

    let error = registry
        .claimed_scopes(&[])
        .expect_err("every opening would reject the seed");

    assert!(matches!(
        error,
        SeedClaimError::ClaimWithoutSlot {
            claimant: "seeder",
            scope_id,
            ..
        } if scope_id == Session::ID
    ));
}
