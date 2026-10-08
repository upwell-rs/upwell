//! Rollback on hook rejection: a candidate-generation `ConfigReload` hook that rejects
//! the proposal aborts the reload before the terminal commit, leaving the complete
//! previous generation — config values included — active.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use upwell_config::{ConfigReloadError, ContainerConfigExt};

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_calls, factory_tokens,
    hook_calls, lock_test_guard, mutated_receivers, proposed_tokens, receiver_ids, receiver_tokens,
    reject_hook_token, reset_controls, set_hook_mutates_receiver, write_probe_config,
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
    let old_config_generation = app.runtime().config_generation();

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

#[tokio::test]
async fn receiver_mutating_hook_rejection_preserves_the_active_receiver() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    // The probe is active, so the hook has a real receiver to mutate: constructed once
    // against token 1, with its receiver-local marker clear.
    let old_runtime_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();
    let old_root = runtime.root().clone();
    let active = old_root
        .get::<ProbeComponent>()
        .expect("the active probe resolves");
    let active_id = Arc::as_ptr(&active) as usize;

    assert_eq!(active.observed, 1);
    assert!(!active.mutated.load(Ordering::SeqCst));

    // The hook mutates whichever receiver it resolves, then rejects this reload's
    // proposed token.
    set_hook_mutates_receiver(true).await;
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

    // The complete previous generation stays active — receiver state and identity
    // included. The candidate receiver the hook mutated was a distinct, discarded
    // instance; this proves the active receiver never observes a candidate run's
    // mutation, not that an external side effect was rolled back.
    assert_eq!(
        runtime.generation(),
        old_runtime_generation,
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

    let current = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the active probe still resolves");

    assert!(
        Arc::ptr_eq(&active, &current),
        "the active receiver identity is unchanged"
    );
    assert_eq!(current.observed, 1, "the active receiver keeps its value");
    assert!(
        !current.mutated.load(Ordering::SeqCst),
        "the active receiver never observes the candidate hook run's mutation"
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

    // The hook ran on a distinct reconstructed candidate receiver: the candidate factory
    // built a fresh probe against the staged proposal, and the hook resolved that
    // instance — never the active one.
    assert_eq!(
        factory_calls(),
        2,
        "the candidate factory reconstructed the probe"
    );
    assert_eq!(
        factory_tokens().await,
        vec![1, 7],
        "the candidate factory resolved the staged proposal"
    );
    assert_eq!(hook_calls(), 1, "the config-reload hook ran exactly once");
    assert_eq!(
        proposed_tokens().await,
        vec![7],
        "the hook observed the proposed token"
    );
    assert_eq!(
        receiver_tokens().await,
        vec![7],
        "the hook's receiver observed the candidate value, not the active value"
    );

    let receiver_ids = receiver_ids().await;
    let mutated_receivers = mutated_receivers().await;

    assert_eq!(
        receiver_ids.len(),
        1,
        "the hook resolved exactly one receiver"
    );
    assert_ne!(
        receiver_ids[0], active_id,
        "the hook's receiver is a distinct instance from the active receiver"
    );

    // The mutation was applied to exactly the receiver the hook resolved: the mutated
    // receiver is recorded only after the mutation, so its entry matches the hook
    // receiver's identity and proposed token while differing from the active receiver.
    assert_eq!(
        mutated_receivers,
        vec![(receiver_ids[0], 7)],
        "the hook mutated exactly its resolved candidate receiver"
    );
    assert_ne!(
        mutated_receivers[0].0, active_id,
        "the mutated receiver is not the active receiver"
    );
}
