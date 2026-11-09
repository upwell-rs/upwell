use upwell_core::{Scope, Singleton};

use super::*;

/// A claimed value.
struct Identity;

#[test]
fn one_claimant_claims_a_seed_once() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Singleton>("endpoint"));
    registry.claim_seed(SeedClaim::of::<Identity, Singleton>("endpoint"));

    assert_eq!(registry.seed_claims.len(), 1);
    assert!(registry.claimed_scopes().is_ok());
}

#[test]
fn two_claimants_of_one_seed_in_one_scope_are_rejected() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Singleton>("first"));
    registry.claim_seed(SeedClaim::of::<Identity, Singleton>("second"));

    let error = registry
        .claimed_scopes()
        .expect_err("two claimants fill one slot twice");

    assert!(matches!(
        error,
        SeedClaimError::DuplicateSeedClaim {
            first: "first",
            second: "second",
            scope_id,
            ..
        } if scope_id == Singleton::ID
    ));
}
