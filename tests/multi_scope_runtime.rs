#![cfg(feature = "daemon")]

//! One component type placed in two disjoint application scopes: each scope builds the
//! factory declared for it, opened through `ScopeOpener`. Scope lists (`scope = [A, B]` on
//! `#[component]` and `#[init]`) build one instance per scope instance, and the invocation
//! accessors reach the placement on the active chain (#136, #139).

extern crate upwell as fork;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::context::{Current, Scoped, ScopedError};
use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{component, methods, scope};
use upwell_test_utils::ok;

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

/// Numbers handed out to [`Listed`] instances.
static LISTED: AtomicUsize = AtomicUsize::new(0);

/// A component the field-injection default builds in both scopes.
#[component(scope = [Left, Right])]
struct Listed {
    #[default]
    number: Numbered,
}

/// The next [`Listed`] number.
struct Numbered(usize);

impl Default for Numbered {
    fn default() -> Self {
        Self(LISTED.fetch_add(1, Ordering::SeqCst))
    }
}

/// A component whose one constructor lists both scopes, recording the [`Listed`] it saw.
#[component]
struct PerRun {
    #[default]
    listed: usize,
}

#[methods]
impl PerRun {
    #[init(scope = [Left, Right])]
    fn from_listed(listed: Arc<Listed>) -> Self {
        Self {
            listed: listed.number.0,
        }
    }
}

/// A singleton reading [`Listed`] from whichever scope the invocation runs in.
#[component]
struct Reader {
    current: Current<Arc<Listed>>,
    left: Scoped<Left, Arc<Listed>>,
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

#[tokio::test]
async fn a_listed_component_builds_in_every_listed_scope() {
    let app = app().await;
    let runtime = app.runtime();

    let left = runtime
        .opener::<Left>()
        .expect("discovery declares the left scope")
        .builder(Left)
        .try_run(|listed: Arc<Listed>, again: Arc<Listed>| async move {
            ok((listed.number.0, Arc::ptr_eq(&listed, &again)))
        })
        .await
        .expect("the left run builds the listed component");
    let right = runtime
        .opener::<Right>()
        .expect("discovery declares the right scope")
        .builder(Right)
        .try_run(|listed: Arc<Listed>, per_run: Arc<PerRun>| async move {
            ok((listed.number.0, per_run.listed == listed.number.0))
        })
        .await
        .expect("the right run builds the listed component and the listed init");

    assert!(left.1, "one instance per scope instance");
    assert_ne!(left.0, right.0, "each scope instance builds its own");
    assert!(
        right.1,
        "the listed init reads the placement of its own scope"
    );
}

#[tokio::test]
async fn accessors_reach_the_placement_on_the_active_chain() {
    let app = app().await;
    let runtime = app.runtime();

    let (left, pinned) = runtime
        .opener::<Left>()
        .expect("discovery declares the left scope")
        .builder(Left)
        .try_run(|listed: Arc<Listed>, reader: Arc<Reader>| async move {
            let current = reader.current.get().await.expect("the left placement");
            let pinned = reader.left.get().await.expect("the pinned left placement");

            ok((
                Arc::ptr_eq(&listed, &current),
                Arc::ptr_eq(&listed, &pinned),
            ))
        })
        .await
        .expect("the left run reads its placement");
    let (right, pinned_elsewhere) = runtime
        .opener::<Right>()
        .expect("discovery declares the right scope")
        .builder(Right)
        .try_run(|listed: Arc<Listed>, reader: Arc<Reader>| async move {
            let current = reader.current.get().await.expect("the right placement");

            ok((
                Arc::ptr_eq(&listed, &current),
                reader.left.get().await.err(),
            ))
        })
        .await
        .expect("the right run reads its placement");

    assert!(
        left && pinned,
        "a left run reads the left instance both ways"
    );
    assert!(right, "a right run reads the right instance");
    assert!(
        matches!(pinned_elsewhere, Some(ScopedError::ScopeNotActive { .. })),
        "a pin to a scope off the chain fails: {pinned_elsewhere:?}"
    );
}
