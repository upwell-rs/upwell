#![cfg(feature = "daemon")]
#![allow(dead_code)]

//! `#[context]` values (#127) and seed claims (#128): one app-defined value seeded into
//! several unrelated scopes, the same consumer reading it from whichever scope its chain
//! holds, and preparation rejecting a required context value no one claims to seed.

extern crate upwell as fork;

use std::sync::Arc;

use upwell::daemon::App;
use upwell::scope::Root;
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
async fn two_claimants_of_one_seed_fail_preparation() {
    let mut builder = builder("context-duplicate").provides_seed::<Identity, Left>();

    builder
        .registry_mut()
        .claim_seed(SeedClaim::of::<Identity, Left>("another"));

    let error = builder
        .build()
        .await
        .expect_err("two claimants fill one slot twice");

    assert!(
        error.to_string().contains("another"),
        "the second claimant is named: {error}"
    );
}
