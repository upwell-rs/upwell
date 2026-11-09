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

    let provided = registry
        .provided(&[slot()])
        .expect("each opener fills its own instances");

    assert!(matches!(
        provided.coverage(TypeId::of::<Identity>(), Session::ID, Reached::Any),
        Coverage::Provided
    ));
}

#[test]
fn a_claim_for_a_scope_without_a_slot_is_rejected() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Session>("seeder"));

    let Err(error) = registry.provided(&[]) else {
        panic!("every opening would reject the seed");
    };

    assert!(matches!(
        error,
        SeedClaimError::ClaimWithoutSlot {
            ref claimant,
            scope_id,
            ..
        } if claimant == "seeder" && scope_id == Session::ID
    ));
}

/// An opener of [`Session`] declaring `seeds`.
fn opener(name: &str, seeds: Vec<TypeDescriptor>) -> OpenerSeeds {
    OpenerSeeds {
        opener: name.to_owned(),
        scope: Session::KEY,
        seeds,
        entries: None,
    }
}

#[test]
fn one_openers_seed_does_not_vouch_for_another_opener() {
    let mut registry = AppRegistry::default();

    registry.declare_opener(opener(
        "seeding endpoint",
        vec![TypeDescriptor::of::<Identity>("Identity")],
    ));
    registry.declare_opener(opener("bare endpoint", Vec::new()));

    let provided = registry.provided(&[slot()]).expect("every seed has a slot");

    assert!(matches!(
        provided.coverage(TypeId::of::<Identity>(), Session::ID, Reached::Any),
        Coverage::MissingIn(opener) if opener.opener == "bare endpoint"
    ));
}

#[test]
fn a_plain_claim_does_not_cover_a_scope_with_declared_openers() {
    let mut registry = AppRegistry::default();

    registry.claim_seed(SeedClaim::of::<Identity, Session>("application"));
    registry.declare_opener(opener("bare endpoint", Vec::new()));

    let provided = registry.provided(&[slot()]).expect("every seed has a slot");

    assert!(matches!(
        provided.coverage(TypeId::of::<Identity>(), Session::ID, Reached::Any),
        Coverage::MissingIn(_)
    ));
}

#[test]
fn an_opener_that_serves_other_entries_is_not_checked_for_an_entry() {
    let served = EntryKey::new("ws/stomp_greeting.greet").expect("a valid entry key");
    let other = EntryKey::new("ws/json_greeting.greet").expect("a valid entry key");
    let mut registry = AppRegistry::default();

    registry.declare_opener(OpenerSeeds {
        entries: Some(vec![served]),
        ..opener(
            "seeding endpoint",
            vec![TypeDescriptor::of::<Identity>("Identity")],
        )
    });
    registry.declare_opener(OpenerSeeds {
        entries: Some(vec![other]),
        ..opener("bare endpoint", Vec::new())
    });

    let provided = registry.provided(&[slot()]).expect("every seed has a slot");

    assert!(matches!(
        provided.coverage(
            TypeId::of::<Identity>(),
            Session::ID,
            Reached::Entry(&served)
        ),
        Coverage::Provided
    ));
    assert!(matches!(
        provided.coverage(TypeId::of::<Identity>(), Session::ID, Reached::Entry(&other)),
        Coverage::MissingIn(opener) if opener.opener == "bare endpoint"
    ));
}
