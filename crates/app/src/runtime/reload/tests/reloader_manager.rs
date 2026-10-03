//! The legacy reloader tracks the current generation's hook manager: after a
//! transactional reload publishes a generation, a direct
//! [`ConfigReloader::reload`](upwell_config::ConfigReloader::reload) runs the newly
//! active generation's hooks against the current root, and hooks of removed
//! generations never run.

use upwell_config::HookOutcome;

use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, hook_calls, lock_test_guard, proposed_tokens,
    receiver_tokens, reset_controls, write_probe_config,
};

#[tokio::test]
async fn legacy_reload_runs_the_current_generations_hooks() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent, and the startup manager the
    // reloader holds carries no probe hook.
    assert!(runtime.root().get::<ProbeComponent>().is_none());

    // Transactional reload #1 activates the probe: the published generation's
    // manager carries its hook, attached to the published root.
    write_probe_config(&config_dir_of(&dir), true, 7);

    let activated = runtime
        .reload_config()
        .await
        .expect("the activation reload succeeds");

    assert!(activated.published, "the activation publishes a generation");
    assert_eq!(hook_calls(), 1, "the activation ran the hook once");

    // Direct legacy reload on a value change that keeps the probe active: the newly
    // active hook must run and resolve its receiver through the current root.
    write_probe_config(&config_dir_of(&dir), true, 8);

    let legacy = app
        .config_reloader()
        .reload()
        .await
        .expect("the legacy reload runs the current hook without a resolver failure");

    assert_eq!(legacy.changed.len(), 1, "only the probe binding changed");
    assert_eq!(
        legacy.hooks.len(),
        1,
        "the newly active hook ran on the legacy reload"
    );
    assert_eq!(legacy.hooks[0].component, "ProbeComponent");
    assert_eq!(legacy.hooks[0].outcome, HookOutcome::Unchanged);
    assert_eq!(hook_calls(), 2, "the hook ran exactly once more");
    assert_eq!(
        proposed_tokens().await,
        vec![7, 8],
        "the legacy run observed the newly proposed value"
    );
    assert_eq!(
        receiver_tokens().await,
        vec![7, 7],
        "both runs resolved the receiver from the published generation's root"
    );

    // Transactional reload #2 deactivates the probe: the published generation's
    // manager no longer carries its hook.
    write_probe_config(&config_dir_of(&dir), false, 9);

    let deactivated = runtime
        .reload_config()
        .await
        .expect("the deactivation reload succeeds");

    assert!(deactivated.published, "the deactivation publishes");
    assert!(
        deactivated.hooks.is_empty(),
        "the removed hook does not run in the deactivating transaction"
    );
    assert_eq!(hook_calls(), 2, "no hook ran while the probe was removed");

    // A direct legacy reload while the probe is removed must not run the old hook.
    write_probe_config(&config_dir_of(&dir), false, 10);

    let removed = app
        .config_reloader()
        .reload()
        .await
        .expect("the legacy reload succeeds while the hook is removed");

    assert_eq!(removed.changed.len(), 1, "only the probe binding changed");
    assert!(
        removed.hooks.is_empty(),
        "the removed hook does not run on the legacy reload"
    );
    assert_eq!(hook_calls(), 2, "no removed hook ran");

    // Transactional reload #3 re-activates the probe with a fresh instance, and a
    // final direct legacy reload runs the replacement hook against the newest root.
    write_probe_config(&config_dir_of(&dir), true, 11);

    let reactivated = runtime
        .reload_config()
        .await
        .expect("the re-activation reload succeeds");

    assert!(reactivated.published, "the re-activation publishes");
    assert_eq!(
        hook_calls(),
        3,
        "the replacement hook ran in the transaction"
    );

    write_probe_config(&config_dir_of(&dir), true, 12);

    let replaced = app
        .config_reloader()
        .reload()
        .await
        .expect("the legacy reload runs the replacement hook");

    assert_eq!(replaced.hooks.len(), 1, "the replacement hook ran");
    assert_eq!(replaced.hooks[0].component, "ProbeComponent");
    assert_eq!(replaced.hooks[0].outcome, HookOutcome::Unchanged);
    assert_eq!(hook_calls(), 4, "the hook ran exactly once more");
    assert_eq!(
        proposed_tokens().await,
        vec![7, 8, 11, 12],
        "each active generation's hook observed its own proposal"
    );
    assert_eq!(
        receiver_tokens().await,
        vec![7, 7, 11, 11],
        "the replacement hook resolved the newest instance from the current root"
    );
}
