//! #291 acceptance: a retained `Dep<dyn Authenticator>` consumer follows the provider
//! switch in both directions without reconstruction, while the fixed
//! [`AuthGate`](super::gate::AuthGate) consumer of the same provider reconstructs in the
//! same transition.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use upwell_di::{ComponentDescriptor, Dep, factories};

use super::fixture::{
    Authenticator, build_auth_app_with, config_dir_of, fail_fallback_factory, lock_test_guard,
    write_auth_config,
};
use super::gate::{AuthGate, GATE_COMPONENT};
use crate::test_support::test_component;
use crate::{App, AppRuntime};

/// A retained singleton that follows the effective authenticator through a live handle.
struct AuthWatcher {
    authenticator: Dep<dyn Authenticator>,
}

test_component!(AuthWatcher, "auth-watcher");

/// A fixed consumer of the watcher, which keeps its instance when the watcher rebinds.
struct WatcherAudit {
    watcher: Arc<AuthWatcher>,
}

test_component!(WatcherAudit, "auth-watcher-audit");

static WATCHER_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

async fn construct_watcher(authenticator: Dep<dyn Authenticator>) -> AuthWatcher {
    WATCHER_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

    AuthWatcher { authenticator }
}

async fn construct_audit(watcher: Arc<AuthWatcher>) -> WatcherAudit {
    WatcherAudit { watcher }
}

static WATCHER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<AuthWatcher>()
    .with_factories(factories!["static" => construct_watcher]);

static AUDIT_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<WatcherAudit>()
    .with_factories(factories!["static" => construct_audit]);

async fn build_watched_app(mode: &str) -> (tempfile::TempDir, App<()>) {
    WATCHER_CONSTRUCTIONS.store(0, Ordering::SeqCst);

    build_auth_app_with(
        mode,
        &GATE_COMPONENT,
        &[&WATCHER_COMPONENT, &AUDIT_COMPONENT],
    )
    .await
}

fn watcher(runtime: &AppRuntime) -> Arc<AuthWatcher> {
    runtime
        .view()
        .root()
        .get::<AuthWatcher>()
        .expect("the watcher resolves from the current root")
}

fn gate_scheme(runtime: &AppRuntime) -> &'static str {
    runtime
        .view()
        .root()
        .get::<AuthGate>()
        .expect("the gate resolves from the current root")
        .scheme
}

async fn switch(dir: &tempfile::TempDir, runtime: &AppRuntime, mode: &str) {
    write_auth_config(&config_dir_of(dir), mode);

    let report = runtime.reload_config().await.expect("the switch commits");

    assert!(report.is_committed(), "switching to {mode} publishes");
}

#[tokio::test]
async fn live_consumer_follows_the_provider_switch_in_both_directions() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_watched_app("primary").await;
    let runtime = app.runtime();
    let original = watcher(runtime);
    let audit = runtime
        .view()
        .root()
        .get::<WatcherAudit>()
        .expect("the audit resolves from the initial root");
    let primary_snapshot = original.authenticator.snapshot();

    assert_eq!(original.authenticator.get().scheme(), "primary");

    switch(&dir, runtime, "fallback").await;

    let after_fallback = watcher(runtime);

    assert!(
        Arc::ptr_eq(&original, &after_fallback),
        "the live consumer keeps its instance"
    );
    assert_eq!(after_fallback.authenticator.get().scheme(), "fallback");
    assert_eq!(
        gate_scheme(runtime),
        "fallback",
        "the fixed consumer reconstructed in the same transition"
    );
    assert_eq!(
        primary_snapshot.scheme(),
        "primary",
        "a snapshot taken before the switch keeps the previous provider"
    );

    switch(&dir, runtime, "primary").await;

    let after_primary = watcher(runtime);
    let retained_audit = runtime
        .view()
        .root()
        .get::<WatcherAudit>()
        .expect("the audit resolves from the latest root");

    assert!(Arc::ptr_eq(&original, &after_primary));
    assert_eq!(after_primary.authenticator.get().scheme(), "primary");
    assert_eq!(gate_scheme(runtime), "primary");
    assert!(
        Arc::ptr_eq(&audit, &retained_audit) && Arc::ptr_eq(&retained_audit.watcher, &original),
        "a fixed dependent of a rebound consumer keeps its instance"
    );
    assert_eq!(
        WATCHER_CONSTRUCTIONS.load(Ordering::SeqCst),
        1,
        "the live consumer was never reconstructed"
    );
}

#[tokio::test]
async fn failed_switch_leaves_the_live_consumer_on_the_previous_provider() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_watched_app("primary").await;
    let runtime = app.runtime();
    let original = watcher(runtime);
    let generation = runtime.generation();

    fail_fallback_factory(true);
    write_auth_config(&config_dir_of(&dir), "fallback");

    let result = runtime.reload_config().await;

    fail_fallback_factory(false);

    assert!(
        result.is_err(),
        "the failing fallback factory aborts the switch"
    );
    assert_eq!(runtime.generation(), generation);
    assert!(Arc::ptr_eq(&original, &watcher(runtime)));
    assert_eq!(
        original.authenticator.get().scheme(),
        "primary",
        "the held live cell still points at the previous provider"
    );

    switch(&dir, runtime, "fallback").await;

    assert_eq!(
        original.authenticator.get().scheme(),
        "fallback",
        "a later successful switch still rebinds the same consumer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_readers_observe_each_switch_exactly_once() {
    const SWITCHES: usize = 6;

    let _guard = lock_test_guard().await;
    let (dir, app) = build_watched_app("primary").await;
    let runtime = app.runtime();
    let original = watcher(runtime);
    let stop = Arc::new(AtomicBool::new(false));
    let fallback_seen = Arc::new(AtomicBool::new(false));
    let reader = {
        let watcher = Arc::clone(&original);
        let stop = Arc::clone(&stop);
        let fallback_seen = Arc::clone(&fallback_seen);

        tokio::spawn(async move {
            let mut last = "primary";
            let mut transitions = 0_usize;

            while !stop.load(Ordering::SeqCst) {
                let scheme = watcher.authenticator.get().scheme();

                if scheme != last {
                    transitions += 1;
                    last = scheme;
                }

                fallback_seen.store(scheme == "fallback", Ordering::SeqCst);
                tokio::task::yield_now().await;
            }

            transitions
        })
    };

    for round in 0..SWITCHES {
        let fallback = round % 2 == 0;
        let mode = if fallback { "fallback" } else { "primary" };

        switch(&dir, runtime, mode).await;

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while fallback_seen.load(Ordering::SeqCst) != fallback {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the concurrent reader observes the committed provider");
    }

    stop.store(true, Ordering::SeqCst);

    let transitions = reader.await.expect("the reader task completes");

    assert_eq!(
        transitions, SWITCHES,
        "the reader saw every switch exactly once, never reverting to a previous provider"
    );
    assert!(Arc::ptr_eq(&original, &watcher(runtime)));
}
