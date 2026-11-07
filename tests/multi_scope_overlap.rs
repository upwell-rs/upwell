#![cfg(feature = "daemon")]
#![allow(dead_code)]

//! A component type placed in a scope and in one of its descendants fails preparation.

extern crate upwell as fork;

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{component, methods, scope};

/// The outer scope.
#[scope(parents = [Root])]
pub struct Outer;

/// A scope opened under [`Outer`].
#[scope(parents = [Outer])]
pub struct Inner;

/// A component placed in both scopes, which share a hierarchy.
#[component(scope = Outer)]
struct Overlapping {
    #[default]
    label: String,
}

#[methods]
impl Overlapping {
    #[init]
    fn in_outer() -> Self {
        Self {
            label: "outer".to_string(),
        }
    }
}

#[methods]
impl Overlapping {
    #[init(scope = Inner)]
    fn in_inner() -> Self {
        Self {
            label: "inner".to_string(),
        }
    }
}

#[tokio::test]
async fn placements_sharing_a_hierarchy_fail_preparation() {
    let error = App::builder("multi-scope-overlap")
        .auto_discover()
        .build()
        .await
        .expect_err("overlapping placements are rejected");

    assert!(
        error.to_string().contains("may not share a hierarchy"),
        "{error}"
    );
}
