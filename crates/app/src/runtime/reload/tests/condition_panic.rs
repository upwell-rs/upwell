//! Panic containment in the reload's condition stage: a panicking condition-fact
//! extraction or descriptor callback, or config condition callback, aborts the reload
//! with the redacted preparation panic error before any candidate graph, root, hook,
//! or publication work, and the reload serializers stay usable for a subsequent reload.

use std::sync::Arc;

use tempfile::TempDir;
use upwell_config::ContainerConfigExt;

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_calls, factory_tokens,
    hook_calls, lock_test_guard, panic_condition_descriptors, panic_condition_evaluation,
    panic_condition_scalars, proposed_tokens, reset_controls, write_probe_config,
};
use crate::App;

/// Drives one panic-contained reload scenario over the probe app built with
/// `baseline_enabled` and token 1: arms the panic control via `set_panic`, stages the
/// `changed` probe config, and asserts the reload fails with the redacted preparation
/// panic error while the complete pre-reload framework state stays active and no
/// candidate factory or hook ran. It then disarms the control and asserts a successful
/// recovery reload that publishes the staged values.
async fn assert_panic_contained_reload_recovers(
    dir: &TempDir,
    app: &App<()>,
    baseline_enabled: bool,
    set_panic: impl Fn(bool),
    changed: (bool, i64),
) {
    let runtime = app.runtime();

    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();
    let factories_before = factory_calls();
    let hooks_before = hook_calls();

    // The armed condition-stage callback panics while the reload holds both
    // serialization leases.
    set_panic(true);

    write_probe_config(&config_dir_of(dir), changed.0, changed.1);

    let error = runtime
        .reload_config()
        .await
        .expect_err("the armed condition-stage callback panics");

    assert!(
        matches!(error, crate::Error::CandidatePreparationPanicked),
        "expected the redacted preparation panic error, got {error:?}"
    );

    // The panic hit before any candidate work: the complete previous generation,
    // config values included, is still active.
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
        runtime
            .root()
            .config::<ProbeConfig>("probe")
            .expect("the probe binding resolves")
            .snapshot()
            .token,
        1,
        "the active config value is unchanged"
    );

    let active = runtime.root().get::<ProbeComponent>();

    assert_eq!(
        active.is_some(),
        baseline_enabled,
        "active component presence still matches the baseline condition"
    );

    if let Some(active) = active {
        assert_eq!(
            active.observed, 1,
            "the active component instance is unchanged"
        );
    }

    assert_eq!(
        factory_calls(),
        factories_before,
        "no candidate construction ran"
    );
    assert_eq!(hook_calls(), hooks_before, "no config-reload hook ran");

    // Disarming proves the reload serializers survived the panic: the same changed
    // source re-stages and publishes.
    set_panic(false);

    let report = runtime
        .reload_config()
        .await
        .expect("the reload succeeds after the panic control resets");

    assert!(report.published, "the recovered reload publishes");
    assert_eq!(
        report.changed.len(),
        1,
        "the recovered reload republishes the changed binding"
    );
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the recovered reload advances the runtime generation"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation + 1,
        "the recovered reload advances the config generation"
    );
    assert_eq!(
        runtime
            .root()
            .config::<ProbeConfig>("probe")
            .expect("the probe binding resolves")
            .snapshot()
            .token,
        changed.1,
        "the recovered reload publishes the staged value"
    );
    assert_eq!(
        runtime
            .root()
            .get::<ProbeComponent>()
            .expect("the recovered generation activates the probe")
            .observed,
        changed.1,
        "the recovered generation constructs from the staged value"
    );

    let mut expected_factory_tokens = Vec::new();

    if baseline_enabled {
        expected_factory_tokens.push(1);
    }

    expected_factory_tokens.push(changed.1);
    assert_eq!(
        factory_tokens().await,
        expected_factory_tokens,
        "only the initial build (if any) and the recovered candidate constructed"
    );
    assert_eq!(
        proposed_tokens().await,
        vec![changed.1],
        "the recovered reload proposed the staged token to the hook"
    );
}

#[tokio::test]
async fn condition_fact_panic_preserves_generations_and_recovers() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;

    assert_panic_contained_reload_recovers(&dir, &app, true, panic_condition_scalars, (true, 7))
        .await;
}

#[tokio::test]
async fn condition_fact_descriptor_panic_preserves_generations_and_recovers() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;

    assert_panic_contained_reload_recovers(
        &dir,
        &app,
        true,
        panic_condition_descriptors,
        (true, 7),
    )
    .await;
}

#[tokio::test]
async fn condition_evaluation_callback_panic_preserves_generations_and_recovers() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    // The probe starts condition-disabled; the staged reload flips the fact the
    // callback declares as its only input, so incremental evaluation re-runs the
    // callback instead of reusing the previous decision.
    let (dir, app) = build_probe_app(false, 1).await;

    assert_panic_contained_reload_recovers(
        &dir,
        &app,
        false,
        panic_condition_evaluation,
        (true, 7),
    )
    .await;
}
