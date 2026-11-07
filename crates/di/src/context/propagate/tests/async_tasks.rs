//! Carrying the context onto tokio tasks.

use crate::context::InvocationContext;

use super::*;

#[tokio::test]
async fn capture_outside_every_context_is_a_typed_error() {
    assert_eq!(
        CapturedContext::capture().err(),
        Some(ContextError::OutsideContext)
    );
}

#[tokio::test]
async fn a_carried_spawn_sees_the_context_and_a_plain_one_does_not() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;

    let (carried, plain) = captured
        .enter(async {
            let carried = context_tokio::spawn(async move { sees(&request) });
            let plain = tokio::spawn(async { sees_nothing() });

            (carried.await, plain.await)
        })
        .await;

    assert!(carried.expect("the carried task completes"));
    assert!(plain.expect("the plain task completes"));
}

#[tokio::test]
async fn a_carried_spawn_keeps_the_target_type() {
    let chain = chain().await;
    let (captured, _request) = captured(&chain).await;

    let http = captured
        .enter(async {
            context_tokio::spawn(async {
                InvocationContext::capture().is_ok_and(|context| {
                    context.target().downcast_ref::<Entry>() == Some(&Entry::Http)
                })
            })
            .await
        })
        .await
        .expect("the carried task completes");

    assert!(http, "the task sees the HTTP entry target");
}

#[tokio::test]
async fn concurrent_carried_tasks_keep_their_own_contexts() {
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

    let seen =
        local
            .run_until(captured.enter(async move {
                context_tokio::spawn_local(async move { sees(&request) }).await
            }))
            .await;

    assert!(seen.expect("the local task completes"));
}

#[tokio::test]
async fn an_aborted_task_releases_its_scope_instance() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let weak = Arc::downgrade(&request);

    drop(request);

    let task = captured.run(|| context_tokio::spawn(std::future::pending::<()>()));

    assert!(
        weak.upgrade().is_some(),
        "the running task holds the instance"
    );

    task.abort();

    assert!(task.await.expect_err("the task was aborted").is_cancelled());
    assert!(weak.upgrade().is_none(), "the instance drops with the task");
}

#[tokio::test]
async fn spawning_outside_every_context_runs_plainly() {
    let task = context_tokio::spawn(async { sees_nothing() });
    let blocking = context_tokio::spawn_blocking(sees_nothing);

    assert!(task.await.expect("the task completes"));
    assert!(blocking.await.expect("the blocking task completes"));
}

#[test]
fn a_carried_spawn_enters_the_spawners_span() {
    let subscriber = tracing_subscriber::registry();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime starts");

    let (spawner, spawned) = tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(async {
            let span = tracing::info_span!("spawner");
            let spawner = span.id();
            let spawned = span
                .in_scope(|| context_tokio::spawn(async { tracing::Span::current().id() }))
                .await
                .expect("the task completes");

            (spawner, spawned)
        })
    });

    assert!(spawner.is_some(), "the subscriber enables the span");
    assert_eq!(spawned, spawner);
}
