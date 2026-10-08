//! The happy path: a changed source constructs the candidate against the staged
//! proposal and publishes exactly one runtime and config generation.

use std::sync::Arc;

use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, factory_calls, hook_calls, lock_test_guard,
    proposed_tokens, reset_controls, write_probe_config,
};

#[tokio::test]
async fn candidate_factory_resolves_proposed_config_before_publication() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent from the active generation and the
    // factory has never run.
    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();

    assert!(old_root.get::<ProbeComponent>().is_none());
    assert_eq!(factory_calls(), 0);

    write_probe_config(&config_dir_of(&dir), true, 7);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(report.changed.len(), 1, "only the probe binding changed");
    assert_eq!(report.changed[0].path, "probe");
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the runtime generation advances exactly once"
    );
    assert_eq!(
        app.runtime().config_generation(),
        old_config_generation + 1,
        "the config generation advances once, at the terminal commit"
    );

    // The new root resolves the activated component, and its factory resolved the
    // staged proposed value through the candidate store: at construction time the
    // terminal commit had not run, so the active store still held the old token.
    let probe = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the activated component resolves from the new root");

    assert_eq!(
        probe.observed, 7,
        "the factory saw the staged proposed value"
    );
    assert_eq!(factory_calls(), 1, "the candidate factory ran exactly once");

    // The old generation's root is untouched by the transaction.
    assert!(old_root.get::<ProbeComponent>().is_none());

    // The reload hook ran against the candidate manager over the staged proposal.
    assert_eq!(report.hooks.len(), 1, "the config-reload hook ran");
    assert_eq!(
        proposed_tokens().await,
        vec![7],
        "the hook observed the proposed value, not the active value"
    );
    assert_eq!(hook_calls(), 1, "the config-reload hook ran exactly once");
}

#[tokio::test]
async fn successive_value_only_changes_reconstruct_the_consumer_and_pin_old_generations() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    // Generation 0: the probe is active, constructed exactly once against token 1.
    let old_runtime_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();
    let root0 = runtime.root().clone();
    let probe0 = root0
        .get::<ProbeComponent>()
        .expect("the initial probe resolves");

    assert_eq!(probe0.observed, 1);
    assert_eq!(factory_calls(), 1);

    // First value-only change: eligibility stays enabled, only the token moves 1 → 2.
    write_probe_config(&config_dir_of(&dir), true, 2);

    let report = runtime
        .reload_config()
        .await
        .expect("first reload succeeds");

    assert!(
        report.published,
        "a value-only change publishes a generation"
    );
    assert_eq!(report.changed.len(), 1, "only the probe binding changed");
    assert_eq!(
        runtime.generation().get(),
        old_runtime_generation.get() + 1,
        "the runtime generation advances exactly once"
    );
    assert_eq!(
        app.runtime().config_generation(),
        old_config_generation + 1,
        "the config generation advances once per published reload"
    );
    assert_eq!(factory_calls(), 2, "exactly one reconstruction per reload");

    let root1 = runtime.root().clone();

    assert!(
        !Arc::ptr_eq(&root0, &root1),
        "the current root Arc is replaced"
    );

    let probe1 = root1
        .get::<ProbeComponent>()
        .expect("the reconstructed probe resolves");

    assert!(
        !Arc::ptr_eq(&probe0, &probe1),
        "the config consumer is reconstructed, not retained"
    );
    assert_eq!(
        probe1.observed, 2,
        "the reconstructed factory resolved the new value"
    );
    assert_eq!(
        probe1.cfg.snapshot().token,
        2,
        "the stored Cfg reads the published value"
    );

    // The pinned generation-0 view still resolves the original instance.
    let pinned0 = root0
        .get::<ProbeComponent>()
        .expect("the pinned initial probe still resolves");

    assert!(
        Arc::ptr_eq(&probe0, &pinned0),
        "the pinned root keeps its own instance"
    );
    assert_eq!(
        pinned0.observed, 1,
        "the pinned instance keeps its generation's value"
    );

    // Second value-only change: 2 → 3.
    write_probe_config(&config_dir_of(&dir), true, 3);

    let report = runtime
        .reload_config()
        .await
        .expect("second reload succeeds");

    assert!(report.published);
    assert_eq!(report.changed.len(), 1);
    assert_eq!(
        runtime.generation().get(),
        old_runtime_generation.get() + 2,
        "the runtime generation advances exactly once per reload"
    );
    assert_eq!(
        app.runtime().config_generation(),
        old_config_generation + 2,
        "the config generation advances exactly once per reload"
    );
    assert_eq!(factory_calls(), 3, "exactly one reconstruction per reload");

    let root2 = runtime.root().clone();

    assert!(
        !Arc::ptr_eq(&root1, &root2),
        "each reload replaces the root Arc"
    );
    assert!(!Arc::ptr_eq(&root0, &root2));

    let probe2 = root2
        .get::<ProbeComponent>()
        .expect("the twice-reconstructed probe resolves");

    assert_eq!(probe2.observed, 3);
    assert_eq!(
        probe2.cfg.snapshot().token,
        3,
        "the stored Cfg reads 3 after the second reload"
    );

    // Both pinned generations remain generation-local: each pinned root still resolves
    // its own instance with its original construction-time value, and the first
    // reload's instance still reads its own generation's value (2) through its stored
    // Cfg after the second reload.
    let pinned1 = root1
        .get::<ProbeComponent>()
        .expect("the pinned first-reload probe still resolves");

    assert!(
        Arc::ptr_eq(&probe1, &pinned1),
        "the pinned first-reload root keeps its own instance"
    );
    assert_eq!(pinned1.observed, 2);
    assert_eq!(
        pinned1.cfg.snapshot().token,
        2,
        "the pinned instance's Cfg cell stays generation-local"
    );

    let pinned0 = root0
        .get::<ProbeComponent>()
        .expect("the pinned initial probe still resolves");

    assert!(Arc::ptr_eq(&probe0, &pinned0));
    assert_eq!(pinned0.observed, 1);
}
