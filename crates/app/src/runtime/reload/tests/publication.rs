//! The happy path: a changed source constructs the candidate against the staged
//! proposal and publishes exactly one runtime and config generation.

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
    let old_config_generation = app.config_reloader().generation();

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
        app.config_reloader().generation(),
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
