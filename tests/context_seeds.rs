#![cfg(feature = "daemon")]
#![allow(dead_code)]

//! `#[context]` values (#127) and seed claims (#128): one app-defined value seeded into
//! several unrelated scopes, the same consumer reading it from whichever scope its chain
//! holds, and preparation rejecting a required context value no one claims to seed.

extern crate upwell as fork;

use std::sync::Arc;

use upwell::daemon::App;
use upwell::scope::{Root, Transient};
use upwell::{AppError, SeedClaim, component, context, scope};
use upwell_test_utils::ok;

/// A unit of work under the root that carries an identity.
#[scope(parents = [Root])]
pub struct Left;

/// Another unit of work under the root, sharing no hierarchy with [`Left`].
#[scope(parents = [Root])]
pub struct Right;

/// A step of a [`Left`] instance.
#[scope(parents = [Left])]
pub struct LeftStep;

/// A tenant's scopes, one of them named like the application root.
mod tenant {
    use upwell::scope;

    /// A tenant's unit of work, deliberately named `Root`.
    #[scope(parents = [upwell::scope::Root])]
    pub struct Root;
}

/// A context of a user scope named `Root`: the macro checks the scope's rank, not its name.
#[context(scopes = [tenant::Root])]
#[derive(Clone)]
pub struct TenantId;

/// The identity a [`Left`] or [`Right`] instance runs as.
#[context(scopes = [Left, Right])]
#[derive(Clone)]
pub struct Identity {
    subject: &'static str,
}

/// Reads the identity from the [`Left`] instance a step runs in, or from its own [`Right`]
/// instance: one component type over both chains.
#[component(scope = [LeftStep, Right])]
struct Badge {
    identity: Identity,
}

/// Reads the identity only when one is seeded, so it needs no claim.
#[component(scope = Right)]
struct MaybeBadge {
    identity: Option<Identity>,
}

/// A plain factory-less component no one claims: not checked.
#[component(scope = Left, default_factory = false, by_value)]
#[derive(Clone)]
struct Plain;

/// Requires [`Plain`].
#[component(scope = LeftStep)]
struct PlainReader {
    plain: Plain,
}

/// Built wherever it is requested, with the identity of that scope.
#[component(scope = Transient)]
struct Stamp {
    identity: Identity,
}

/// Requests a [`Stamp`] in a [`Right`] instance.
#[component(scope = Right)]
struct StampReader {
    stamp: Arc<Stamp>,
}

/// The builder with the three scopes and the context.
fn builder(name: &str) -> upwell::AppBuilder<upwell::daemon::Rpc> {
    App::builder(name)
        .scope::<Left>()
        .scope::<Right>()
        .scope::<LeftStep>()
        .component::<Identity>()
}

#[tokio::test]
async fn one_consumer_reads_the_context_from_either_chain() {
    let app = builder("context-either-chain")
        .component::<Badge>()
        .provides_seed::<Identity, Left>()
        .provides_seed::<Identity, Right>()
        .build()
        .await
        .expect("every required placement is claimed");
    let runtime = app.runtime();
    let rights = runtime.opener::<Right>().expect("right is declared");
    let lefts = runtime.opener::<Left>().expect("left is declared");
    let steps = runtime.opener::<LeftStep>().expect("the step is declared");

    let from_right = rights
        .builder(Right)
        .seed(Identity { subject: "right" })
        .try_run(|badge: Arc<Badge>| async move { ok(badge.identity.subject) })
        .await
        .expect("the right instance runs");
    let left = lefts
        .builder(Left)
        .seed(Identity { subject: "left" })
        .open_on_demand()
        .await
        .expect("the left instance opens");
    let from_left = steps
        .builder(LeftStep)
        .under(&left)
        .try_run(|badge: Arc<Badge>| async move { ok(badge.identity.subject) })
        .await
        .expect("the step runs under the left instance");

    assert_eq!(from_right, "right");
    assert_eq!(from_left, "left");
}

#[tokio::test]
async fn a_required_context_without_a_claim_fails_preparation() {
    let error = builder("context-unclaimed")
        .component::<Badge>()
        .provides_seed::<Identity, Left>()
        .build()
        .await
        .expect_err("the right placement is unclaimed");
    let message = error.to_string();

    assert!(
        matches!(&error, upwell::daemon::Error::App(app) if matches!(**app, AppError::SeedClaim(_))),
        "a typed seed-claim error: {error:?}"
    );
    assert!(message.contains("Badge"), "{message}");
    assert!(message.contains("Identity"), "{message}");
    assert!(message.contains("Right"), "{message}");
}

#[tokio::test]
async fn an_optional_context_needs_no_claim() {
    builder("context-optional")
        .component::<MaybeBadge>()
        .build()
        .await
        .expect("an optional dependency is not checked");
}

#[tokio::test]
async fn a_plain_seed_no_one_claims_is_not_checked() {
    builder("context-plain")
        .component::<Plain>()
        .component::<PlainReader>()
        .build()
        .await
        .expect("a plain factory-less component keeps its old rules");
}

#[tokio::test]
async fn two_openers_may_claim_one_seed() {
    let mut builder = builder("context-two-openers")
        .component::<Badge>()
        .provides_seed::<Identity, Left>()
        .provides_seed::<Identity, Right>();

    builder
        .registry_mut()
        .claim_seed(SeedClaim::of::<Identity, Right>("another opener"));

    builder
        .build()
        .await
        .expect("each opener fills its own instances of the scope");
}

#[tokio::test]
async fn a_claim_for_a_scope_without_a_slot_fails_preparation() {
    let error = builder("context-claim-without-slot")
        .provides_seed::<Identity, LeftStep>()
        .build()
        .await
        .expect_err("the step has no identity slot, so every opening would reject the seed");
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(message.contains("LeftStep"), "{message}");
}

/// A transient component cannot hold a scoped seed at all: scope validation rejects the edge
/// before seed claims are checked, which in turn check a transient component's requirements in
/// every scope that requests it.
#[tokio::test]
async fn a_transient_requirement_of_a_scoped_seed_is_rejected() {
    let error = builder("context-transient")
        .component::<Stamp>()
        .component::<StampReader>()
        .provides_seed::<Identity, Left>()
        .build()
        .await
        .expect_err("a right instance would build the stamp without an identity");
    let message = error.to_string();

    assert!(message.contains("Stamp"), "{message}");
    assert!(message.contains("Identity"), "{message}");
}
