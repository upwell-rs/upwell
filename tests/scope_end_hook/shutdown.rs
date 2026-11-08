//! Pending end hooks survive the end of the work that ended the instance.

use upwell::ScopeEndKind;

use super::*;

#[tokio::test]
async fn shutting_down_delivers_the_end_of_a_scope_that_ran_last() {
    let app = app("scope-end-shutdown").await;

    opener(&app)
        .builder(ended(40))
        .run(|| async {})
        .await
        .expect("the run succeeds");
    app.shutdown_handle().shutdown();
    app.run().await.expect("the app shuts down");

    let ends = ends_of(40);

    assert_eq!(
        ends.len(),
        1,
        "the drain delivered the end before returning"
    );
    assert_eq!(ends[0].kind, ScopeEndKind::Completed);
}

#[tokio::test]
async fn draining_waits_for_an_observer_that_is_still_running() {
    let app = app("scope-end-drain").await;

    opener(&app)
        .builder(ended(GATED_DRAIN))
        .run(|| async {})
        .await
        .expect("the run succeeds");

    assert!(
        !app.runtime()
            .drain_scope_ends(Duration::from_millis(50))
            .await,
        "the gated observer is still running"
    );

    RELEASE_DRAIN.notify_one();

    assert!(app.runtime().drain_scope_ends(Duration::from_secs(5)).await);
    assert_eq!(ends_of(GATED_DRAIN).len(), 1);
}
