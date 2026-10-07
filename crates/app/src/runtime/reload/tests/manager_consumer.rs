//! A component that stores its generation's [`HookManager`] — injected through
//! ordinary DI at construction — stays bound to the generation that constructed it:
//! every publication reconstructs the current holder with the newly published
//! manager, while a holder retained from an older generation keeps resolving its own
//! pinned root's receiver. It neither retargets to the current generation nor loses
//! its resolver context while its view is pinned.
//!
//! Every firing goes through a holder's stored manager — never
//! [`AppRuntime::hooks`](crate::AppRuntime::hooks) or the config reloader's manager —
//! so the assertions cover exactly the handle an ordinary component would keep.

use std::sync::Arc;

use super::fixture::{
    ManagerConsumer, ProbeComponent, build_probe_app_with_manager_consumer, config_dir_of,
    fire_manager_probe, lock_test_guard, manager_probe_ids, manager_probe_tokens, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn stored_manager_holders_resolve_their_own_generation_across_publications() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app_with_manager_consumer(true, 1).await;
    let runtime = app.runtime();

    // Generation 0: the holder and the probe are constructed once, and the holder
    // stores generation 0's manager.
    let root0 = runtime.root().clone();
    let holder0 = root0
        .get::<ManagerConsumer>()
        .expect("the initial holder resolves");
    let probe0 = root0
        .get::<ProbeComponent>()
        .expect("the initial probe resolves");

    assert_eq!(probe0.observed, 1);

    // Firing through the holder's stored manager resolves generation 0's receiver.
    fire_manager_probe(&holder0.hooks)
        .await
        .expect("the initial holder's manager resolves its own generation");
    assert_eq!(manager_probe_tokens().await, vec![1]);
    assert_eq!(
        manager_probe_ids().await,
        vec![Arc::as_ptr(&probe0) as usize],
        "the initial holder resolved its own generation's instance"
    );

    // Publication 1 → 2: the current holder Arc is reconstructed with the newly
    // published generation's manager.
    write_probe_config(&config_dir_of(&dir), true, 2);

    runtime
        .reload_config()
        .await
        .expect("the first reload succeeds");

    let root1 = runtime.root().clone();

    assert!(!Arc::ptr_eq(&root0, &root1), "the root Arc is replaced");

    let holder1 = root1
        .get::<ManagerConsumer>()
        .expect("the reconstructed holder resolves");

    assert!(
        !Arc::ptr_eq(&holder0, &holder1),
        "the holder is reconstructed, not retained, by each publication"
    );

    let probe1 = root1
        .get::<ProbeComponent>()
        .expect("the reloaded probe resolves");

    assert_eq!(probe1.observed, 2);

    // The current holder's stored manager resolves the current receiver.
    fire_manager_probe(&holder1.hooks)
        .await
        .expect("the current holder's manager resolves the current generation");
    assert_eq!(manager_probe_tokens().await, vec![1, 2]);
    assert_eq!(
        manager_probe_ids().await.last(),
        Some(&(Arc::as_ptr(&probe1) as usize)),
        "the current holder resolved the current instance"
    );

    // The retained generation-0 holder is still bound to its own pinned generation:
    // it resolves the old receiver — it did not retarget to the current root and did
    // not lose its resolver context.
    fire_manager_probe(&holder0.hooks)
        .await
        .expect("the retained holder's manager still resolves its own generation");
    assert_eq!(manager_probe_tokens().await, vec![1, 2, 1]);
    assert_eq!(
        manager_probe_ids().await.last(),
        Some(&(Arc::as_ptr(&probe0) as usize)),
        "the retained holder resolved its own generation's instance"
    );

    // Publication 2 → 3.
    write_probe_config(&config_dir_of(&dir), true, 3);

    runtime
        .reload_config()
        .await
        .expect("the second reload succeeds");

    let root2 = runtime.root().clone();

    assert!(!Arc::ptr_eq(&root1, &root2));

    let holder2 = root2
        .get::<ManagerConsumer>()
        .expect("the twice-reconstructed holder resolves");

    assert!(
        !Arc::ptr_eq(&holder1, &holder2),
        "each publication reconstructs the holder"
    );

    let probe2 = root2
        .get::<ProbeComponent>()
        .expect("the twice-reloaded probe resolves");

    assert_eq!(probe2.observed, 3);

    // The current holder's stored manager resolves the newest receiver.
    fire_manager_probe(&holder2.hooks)
        .await
        .expect("the current holder's manager resolves the newest generation");
    assert_eq!(manager_probe_tokens().await, vec![1, 2, 1, 3]);
    assert_eq!(
        manager_probe_ids().await.last(),
        Some(&(Arc::as_ptr(&probe2) as usize)),
        "the current holder resolved the newest instance"
    );

    // Both retained holders remain bound to their own generations after the second
    // publication: each still resolves its own instance.
    fire_manager_probe(&holder1.hooks)
        .await
        .expect("the first-reload holder's manager still resolves its own generation");
    assert_eq!(manager_probe_tokens().await, vec![1, 2, 1, 3, 2]);
    assert_eq!(
        manager_probe_ids().await.last(),
        Some(&(Arc::as_ptr(&probe1) as usize)),
        "the first-reload holder resolved its own generation's instance"
    );

    fire_manager_probe(&holder0.hooks)
        .await
        .expect("the initial holder's manager still resolves its own generation");
    assert_eq!(manager_probe_tokens().await, vec![1, 2, 1, 3, 2, 1]);
    assert_eq!(
        manager_probe_ids().await.last(),
        Some(&(Arc::as_ptr(&probe0) as usize)),
        "the initial holder resolved its own generation's instance"
    );
}
