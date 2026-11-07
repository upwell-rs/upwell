//! Plans of the work `ScopeBuilder::run` runs, one per target types and generation.

use std::sync::Arc;

use super::*;

/// Runs work injecting nothing in a session of `opener`.
async fn run_empty(sessions: &ScopeOpener<Session>) {
    sessions
        .builder(Session)
        .run(|| async {})
        .await
        .expect("the run succeeds");
}

/// Runs work with `Option<Arc<Visitor>>` as its one target in a session of `opener`.
async fn run_optional(sessions: &ScopeOpener<Session>) {
    sessions
        .builder(Session)
        .run(|_visitor: Option<Arc<Visitor>>| async {})
        .await
        .expect("the run succeeds");
}

fn run_plans(view: &RuntimeView) -> usize {
    view.scope_plan().layouts.run_plans()
}

#[tokio::test]
async fn work_with_the_same_targets_shares_one_plan() {
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let view = runtime.view();

    assert_eq!(run_plans(&view), 0);

    run_empty(&sessions).await;
    run_empty(&sessions).await;

    sessions
        .builder(Session)
        .run(|| async { 1 })
        .await
        .expect("another closure with the same targets runs");

    assert_eq!(run_plans(&view), 1);

    run_optional(&sessions).await;
    run_optional(&sessions).await;

    assert_eq!(run_plans(&view), 2);
}

#[tokio::test]
async fn a_new_generation_plans_work_again() {
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let first = runtime.view();

    run_empty(&sessions).await;

    let second = publish_next(&runtime).await;

    assert_eq!(run_plans(&second), 0);

    run_empty(&sessions).await;

    assert_eq!(run_plans(&first), 1);
    assert_eq!(run_plans(&second), 1);
}

#[tokio::test]
async fn a_scope_run_is_a_future_that_can_be_joined_and_boxed() {
    use futures::FutureExt;

    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let runs = (0..3).map(|index| sessions.builder(Session).run(move || async move { index }));
    let joined = futures::future::join_all(runs).await;
    let boxed = sessions.builder(Session).run(|| async { 9 }).boxed();

    assert_eq!(
        joined
            .into_iter()
            .map(|run| run.expect("the run succeeds"))
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(boxed.await.expect("the run succeeds"), 9);
}
