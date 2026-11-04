//! Carrying the context onto tokio tasks.

use super::*;

#[tokio::test]
async fn capture_outside_every_context_is_a_typed_error() {
    assert_eq!(
        CapturedContext::capture().err(),
        Some(ContextError::OutsideContext)
    );
}

#[tokio::test]
async fn a_captured_spawn_carries_the_context_and_a_plain_one_does_not() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let entered = captured.clone();

    let (carried, plain) = entered
        .enter(async {
            let carried = spawn({
                let request = Arc::clone(&request);

                async move { sees(&request) }
            });
            let plain = tokio::spawn(async { sees_nothing() });

            (carried.await, plain.await)
        })
        .await;

    assert!(carried.expect("the carried task completes"));
    assert!(plain.expect("the plain task completes"));
    assert!(
        captured
            .spawn(async move { sees(&request) })
            .await
            .expect("the captured task completes")
    );
}

#[tokio::test]
async fn concurrent_captured_tasks_keep_their_own_contexts() {
    let chain = chain().await;
    let mut tasks = tokio::task::JoinSet::new();

    for _ in 0..8 {
        let (captured, request) = captured(&chain).await;

        tasks.spawn(captured.enter(async move {
            for _ in 0..4 {
                tokio::task::yield_now().await;

                assert!(sees(&request));
            }
        }));
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("every task sees its own context");
    }
}

#[tokio::test]
async fn a_local_task_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let local = tokio::task::LocalSet::new();

    let seen = local
        .run_until(async {
            let carried = captured.clone().spawn_local(async move { sees(&request) });
            let free = captured.enter(async { spawn_local(async { innermost().is_ok() }).await });

            (carried.await, free.await)
        })
        .await;

    assert!(seen.0.expect("the local task completes"));
    assert!(seen.1.expect("the free local task completes"));
}

#[tokio::test]
async fn an_aborted_task_releases_its_scope_instance() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let weak = Arc::downgrade(&request);

    drop(request);

    let task = captured.spawn(std::future::pending::<()>());

    assert!(
        weak.upgrade().is_some(),
        "the running task holds the instance"
    );

    task.abort();

    assert!(task.await.expect_err("the task was aborted").is_cancelled());
    assert!(weak.upgrade().is_none(), "the instance drops with the task");
}

#[tokio::test]
async fn the_free_spawn_runs_plainly_outside_every_context() {
    let plain = spawn(async { sees_nothing() });

    assert!(plain.await.expect("the task completes"));
}
