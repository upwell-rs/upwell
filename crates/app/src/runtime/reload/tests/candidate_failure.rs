//! Rollback on candidate reconstruction failure: a condition-enabled factory that
//! fails after resolving the staged config aborts the reload before any hook or
//! commit, leaving the complete previous generation active.

use std::sync::Arc;

use upwell_config::ContainerConfigExt;

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_calls, factory_tokens,
    fail_factory_after_staging, hook_calls, lock_test_guard, proposed_tokens, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn candidate_reconstruction_failure_preserves_config_and_graph() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent and the active config holds token 1.
    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();

    assert!(old_root.get::<ProbeComponent>().is_none());

    // The condition enables the candidate factory, and the factory fails after
    // resolving the staged value.
    fail_factory_after_staging(true).await;

    write_probe_config(&config_dir_of(&dir), true, 7);

    let error = runtime
        .reload_config()
        .await
        .expect_err("the candidate factory fails after staging");

    assert!(
        matches!(error, crate::Error::Di(_)),
        "expected a DI error from the candidate factory, got {error:?}"
    );

    // The same unchanged state as the hook-rejection rollback.
    assert_eq!(
        runtime.generation(),
        old_generation,
        "the runtime generation is unchanged"
    );
    assert_eq!(
        app.runtime().config_generation(),
        old_config_generation,
        "the config generation is unchanged"
    );
    assert!(
        Arc::ptr_eq(&old_root, &runtime.root()),
        "the root identity is unchanged"
    );
    assert_eq!(
        runtime
            .root()
            .config::<ProbeConfig>("probe")
            .expect("the probe binding resolves")
            .snapshot()
            .token,
        1,
        "the active config value is unchanged"
    );
    assert!(
        runtime.root().get::<ProbeComponent>().is_none(),
        "the candidate-only component never becomes active"
    );

    // The factory ran exactly once, against the staged proposal, and no hook ran.
    assert_eq!(factory_calls(), 1, "the candidate factory ran exactly once");
    assert_eq!(
        factory_tokens().await,
        vec![7],
        "the factory recorded the proposed token"
    );
    assert_eq!(hook_calls(), 0, "no config-reload hook ran");
    assert!(
        proposed_tokens().await.is_empty(),
        "no proposal reached a hook"
    );
}
