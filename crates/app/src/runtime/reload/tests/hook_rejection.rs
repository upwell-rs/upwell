//! Rollback on hook rejection: a candidate-generation `ConfigReload` hook that rejects
//! the proposal aborts the reload before the terminal commit, leaving the complete
//! previous generation — config values included — active.

use std::sync::Arc;

use upwell_config::{ConfigReloadError, ContainerConfigExt};

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_calls, factory_tokens,
    hook_calls, lock_test_guard, proposed_tokens, reject_hook_token, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn hook_rejection_preserves_config_and_graph() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent and the active config holds token 1.
    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();

    assert!(old_root.get::<ProbeComponent>().is_none());
    assert_eq!(
        old_root
            .config::<ProbeConfig>("probe")
            .expect("the probe binding resolves")
            .snapshot()
            .token,
        1
    );

    // The hook rejects exactly the token this reload proposes.
    reject_hook_token(7).await;

    write_probe_config(&config_dir_of(&dir), true, 7);

    let error = runtime
        .reload_config()
        .await
        .expect_err("the hook rejects the proposed token");

    match error {
        crate::Error::ConfigReload(error) => match *error {
            ConfigReloadError::Hook { component, .. } => {
                assert_eq!(component, "ProbeComponent", "the probe hook rejected");
            }
            other => panic!("expected a hook rejection, got {other:?}"),
        },
        other => panic!("expected a config-reload error, got {other:?}"),
    }

    // The complete previous generation stays active.
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
    assert!(
        runtime.root().get::<ProbeComponent>().is_none(),
        "the candidate-only component never becomes active"
    );

    // The candidate really was built and the hook really ran before the rollback.
    assert_eq!(
        factory_calls(),
        1,
        "the candidate factory constructed the probe"
    );
    assert_eq!(
        factory_tokens().await,
        vec![7],
        "the factory resolved the staged proposal"
    );
    assert_eq!(hook_calls(), 1, "the config-reload hook ran exactly once");
    assert_eq!(
        proposed_tokens().await,
        vec![7],
        "the hook observed the proposed token"
    );
}
