//! Cancellation before the terminal commit: aborting a reload that is parked inside
//! candidate construction must release the reload lease and the runtime writer, so a
//! later reload completes — and the aborted attempt leaves no state behind.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use upwell_config::ContainerConfigExt;

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_calls, factory_parked,
    factory_tokens, lock_test_guard, park_factory_after_staging, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn cancelled_precommit_attempt_releases_serializers() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent and the active config holds token 1.
    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();

    assert!(old_root.get::<ProbeComponent>().is_none());

    // The candidate factory parks after resolving the staged value, holding both the
    // reload lease and the runtime writer while the test cancels the attempt.
    park_factory_after_staging(true).await;

    write_probe_config(&config_dir_of(&dir), true, 7);

    let task_runtime = runtime.clone();
    let task = tokio::spawn(async move { task_runtime.reload_config().await });

    timeout(Duration::from_secs(5), factory_parked())
        .await
        .expect("the reload parks inside candidate construction after staging");

    task.abort();

    let cancelled = task
        .await
        .expect_err("the aborted reload task does not complete");

    assert!(
        cancelled.is_cancelled(),
        "the reload was cancelled, not failed"
    );

    // The aborted attempt left the previous generation completely intact.
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
    assert_eq!(factory_calls(), 1, "the parked factory ran exactly once");
    assert_eq!(
        factory_tokens().await,
        vec![7],
        "the parked factory had resolved the staged value"
    );

    // A later reload completes: the cancellation released both serializers.
    park_factory_after_staging(false).await;

    write_probe_config(&config_dir_of(&dir), true, 9);

    let report = timeout(Duration::from_secs(5), runtime.reload_config())
        .await
        .expect("the later reload completes after the aborted attempt released both serializers")
        .expect("the later reload completes");

    assert!(report.published, "the later reload publishes a generation");
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the aborted attempt consumed no runtime generation"
    );
    assert_eq!(
        app.runtime().config_generation(),
        old_config_generation + 1,
        "the aborted attempt consumed no config generation"
    );
    assert_eq!(
        runtime
            .root()
            .get::<ProbeComponent>()
            .expect("the later reload activates the component")
            .observed,
        9,
        "the later reload constructed from its own staged value"
    );
}
