//! The no-op path: an unchanged source skips every candidate stage and moves no
//! generation.

use std::sync::Arc;

use upwell_config::ConfigReload;

use super::fixture::{build_probe_app, factory_calls, hook_calls, lock_test_guard, reset_controls};

#[tokio::test]
async fn unchanged_reload_skips_candidate_work() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (_dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();
    let factories_before = factory_calls();
    let hooks_before = hook_calls();

    assert!(
        runtime.hooks().has::<ConfigReload>(),
        "the probe hook is registered, so a zero hook count is meaningful"
    );

    // No source edit: the reload must be a true no-op.
    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(!report.published, "an unchanged source publishes nothing");
    assert!(report.changed.is_empty());
    assert!(report.hooks.is_empty());
    assert_eq!(
        runtime.generation(),
        old_generation,
        "the runtime generation is unchanged"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation,
        "the config generation is unchanged"
    );
    assert!(
        Arc::ptr_eq(&old_root, &runtime.root()),
        "the root identity is unchanged"
    );
    assert_eq!(
        factory_calls(),
        factories_before,
        "no candidate construction ran"
    );
    assert_eq!(hook_calls(), hooks_before, "no config-reload hook ran");
}
