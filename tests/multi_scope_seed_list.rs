#![cfg(feature = "daemon")]
#![allow(dead_code)]

//! A seed has one destination: a manually provided component listing two scopes fails
//! preparation, because `ScopeBuilder::seed` addresses a type, not a scope (#139).

extern crate upwell as fork;

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{component, scope};

/// The first of two disjoint scopes.
#[scope(parents = [Root])]
pub struct Left;

/// The second of two disjoint scopes.
#[scope(parents = [Root])]
pub struct Right;

/// A seed slot declared in both scopes.
#[component(scope = [Left, Right], default_factory = false)]
struct Identity {}

#[tokio::test]
async fn a_seed_listing_two_scopes_fails_preparation() {
    let error = App::builder("multi-scope-seed-list")
        .auto_discover()
        .build()
        .await
        .expect_err("a seed in two scopes is rejected");
    let message = error.to_string();

    assert!(message.contains("Identity"), "{message}");
    assert!(
        message.contains("Left") && message.contains("Right"),
        "{message}"
    );
}
