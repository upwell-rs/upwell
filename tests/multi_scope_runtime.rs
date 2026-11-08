#![cfg(feature = "daemon")]

//! One component type placed in two disjoint application scopes: each scope builds the
//! factory declared for it, opened through `ScopeOpener`.

extern crate upwell as fork;

use std::sync::Arc;

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{component, methods, scope};

/// The first of two disjoint scopes under the root.
#[scope(parents = [Root])]
pub struct Left;

/// The second of two disjoint scopes under the root.
#[scope(parents = [Root])]
pub struct Right;

/// A component built by a different factory in each scope.
#[component(scope = Left)]
struct Worker {
    #[default]
    label: String,
}

#[methods]
impl Worker {
    #[init]
    fn in_left() -> Self {
        Self {
            label: "left".to_string(),
        }
    }
}

#[methods]
impl Worker {
    #[init(scope = Right)]
    fn in_right() -> Self {
        Self {
            label: "right".to_string(),
        }
    }
}

/// A successful result of work that cannot fail.
fn ok<T>(value: T) -> Result<T, std::convert::Infallible> {
    Ok(value)
}

async fn app() -> App {
    App::builder("multi-scope-runtime")
        .auto_discover()
        .build()
        .await
        .expect("the application builds")
}

#[tokio::test]
async fn each_scope_builds_the_factory_declared_for_it() {
    let app = app().await;

    let left = app
        .runtime()
        .opener::<Left>()
        .expect("discovery declares the left scope")
        .builder(Left)
        .try_run(|worker: Arc<Worker>| async move { ok(worker.label.clone()) })
        .await
        .expect("the left run succeeds");
    let right = app
        .runtime()
        .opener::<Right>()
        .expect("discovery declares the right scope")
        .builder(Right)
        .try_run(|worker: Arc<Worker>| async move { ok(worker.label.clone()) })
        .await
        .expect("the right run succeeds");

    assert_eq!(left, "left", "the left scope builds the inherited factory");
    assert_eq!(right, "right", "the right scope builds its own factory");
}
