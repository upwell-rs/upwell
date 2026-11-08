//! Instances a run ends: completed, failed and cancelled, and what the observer may not do.

use upwell::ScopeEndKind;

use super::*;

#[tokio::test]
async fn a_completed_run_ends_once_with_only_its_loggable_params() {
    let _guard = install();
    let app = app("scope-end-completed").await;
    let opener = opener(&app);
    let run = opener.builder(ended(1)).capture_logs().run(|| async {});
    let stream = run.logs();

    run.await.expect("the run succeeds");
    messages(stream).await;

    let ends = ends_of(1);

    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].kind, ScopeEndKind::Completed);
    assert_eq!(ends[0].scope, <Ended as upwell::Scope>::ID);
    assert_eq!(ends[0].params.len(), 1);
    assert_eq!(ends[0].params[0].value, "1");
}

#[tokio::test]
async fn a_failed_run_ends_as_failed() {
    let app = app("scope-end-failed").await;
    let failing = app
        .runtime()
        .opener::<Failing>()
        .expect("discovery declares the failing scope");
    let outcome = failing
        .builder(Failing { tag: 2 })
        .run(|_broken: Arc<Broken>| async {})
        .await;

    assert!(outcome.is_err());
    assert_eq!(end_of(2).await.kind, ScopeEndKind::Failed);
}

#[tokio::test]
async fn a_cancelled_run_ends_as_cancelled() {
    let app = app("scope-end-cancelled").await;
    let opener = opener(&app);
    let run = opener.builder(ended(3)).run(std::future::pending::<()>);

    tokio::time::timeout(Duration::from_millis(20), run)
        .await
        .expect_err("the run never finishes by itself");

    assert_eq!(end_of(3).await.kind, ScopeEndKind::Cancelled);
}

#[tokio::test]
async fn an_observer_runs_before_the_log_closes_and_its_events_are_captured() {
    let _guard = install();
    let app = app("scope-end-log-order").await;
    let opener = opener(&app);
    let run = opener
        .builder(ended(4))
        .capture_logs()
        .run(|| async { tracing::info!("working") });
    let stream = run.logs();

    run.await.expect("the run succeeds");

    assert_eq!(messages(stream).await, ["working", "observed end"]);
    assert_eq!(ends_of(4).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_subscriber_still_captures_the_observer_on_a_multi_thread_runtime() {
    let _guard = install();
    let app = app("scope-end-multi-thread").await;
    let opener = opener(&app);
    let run = opener
        .builder(ended(5))
        .capture_logs()
        .run(|| async { tracing::info!("working") });
    let stream = run.logs();

    run.await.expect("the run succeeds");

    assert_eq!(messages(stream).await, ["working", "observed end"]);
}

#[tokio::test]
async fn a_panicking_observer_neither_fails_the_run_nor_silences_the_others() {
    let app = app("scope-end-panic").await;
    let outcome = opener(&app)
        .builder(ended(PANICKING))
        .run(|| async { 7 })
        .await;

    assert_eq!(outcome.expect("the run succeeds"), 7);
    assert_eq!(end_of(PANICKING).await.kind, ScopeEndKind::Completed);
}

#[tokio::test]
async fn an_observer_cannot_delay_the_run_or_keep_its_components_alive() {
    let app = app("scope-end-delay").await;
    let before = DROPS.load(Ordering::SeqCst);

    opener(&app)
        .builder(ended(GATED))
        .run(|_probe: Arc<DropProbe>| async {})
        .await
        .expect("the run returns while its observer still waits");

    assert_eq!(
        DROPS.load(Ordering::SeqCst) - before,
        1,
        "the instance's components dropped before the observer finished"
    );
    assert!(ends_of(GATED).is_empty());

    RELEASE.notify_one();

    assert_eq!(end_of(GATED).await.kind, ScopeEndKind::Completed);
}
