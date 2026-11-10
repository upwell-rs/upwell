#![cfg(feature = "daemon")]
#![allow(dead_code)]

//! A scope list naming a scope and one of its descendants fails preparation, as two scoped
//! factories would.

extern crate upwell as fork;

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{component, scope};

/// The outer scope.
#[scope(parents = [Root])]
pub struct Outer;

/// A scope opened under [`Outer`].
#[scope(parents = [Outer])]
pub struct Inner;

/// A component listing two scopes that share a hierarchy.
#[component(scope = [Inner, Outer])]
struct Overlapping {}

#[tokio::test]
async fn a_list_sharing_a_hierarchy_fails_preparation() {
    let error = App::builder("multi-scope-list-overlap")
        .auto_discover()
        .build()
        .await
        .expect_err("overlapping listed scopes are rejected");
    let message = error.to_string();

    assert!(message.contains("may not share a hierarchy"), "{message}");
    assert!(
        message.contains("Outer") && message.contains("Inner"),
        "{message}"
    );
}
