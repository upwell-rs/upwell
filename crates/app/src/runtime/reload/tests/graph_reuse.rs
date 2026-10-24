//! A reload whose changes leave every condition's eligibility unchanged shares the base
//! generation's effective graph instead of rebuilding it; a reload that flips eligibility
//! rebuilds it.

use super::fixture::{
    build_probe_app, config_dir_of, lock_test_guard, reset_controls, write_probe_config,
};

#[tokio::test]
async fn unchanged_eligibility_shares_the_base_graph() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();
    let before = runtime.view();

    write_probe_config(&config_dir_of(&dir), true, 2);

    let report = runtime
        .reload_config()
        .await
        .expect("the token-only reload commits");
    let after = runtime.view();

    assert!(report.is_committed(), "the token change publishes");
    assert!(
        after
            .effective_graph()
            .shares_data_with(before.effective_graph()),
        "an eligibility-preserving reload reuses the base graph"
    );
    assert!(
        !std::ptr::eq(
            after.effective_graph().as_ref(),
            before.effective_graph().as_ref()
        ),
        "the committed generation still owns a distinct graph instance"
    );
}

#[tokio::test]
async fn flipped_eligibility_rebuilds_the_graph() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();
    let before = runtime.view();

    write_probe_config(&config_dir_of(&dir), false, 1);

    let report = runtime
        .reload_config()
        .await
        .expect("the disabling reload commits");
    let after = runtime.view();

    assert!(report.is_committed(), "the condition flip publishes");
    assert!(
        !after
            .effective_graph()
            .shares_data_with(before.effective_graph()),
        "an eligibility change rebuilds the graph"
    );
}
