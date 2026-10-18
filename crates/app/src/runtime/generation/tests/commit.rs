//! Committing prepared candidates: condition state and the compatibility callback.

use super::*;

#[tokio::test]
async fn committed_condition_state_survives_initial_commit_and_publication() {
    let state = condition_state(true);
    let direct = condition_registry()
        .evaluate_conditions(condition_facts(), &condition_snapshot(true))
        .expect("direct evaluation succeeds");
    let eligible_ids = |evaluation: &crate::AppConditionEvaluation| {
        evaluation
            .evaluation()
            .eligible_registry()
            .components
            .iter()
            .map(|component| component.id)
            .collect::<Vec<_>>()
    };

    let coordinator = coordinator_with_condition(Arc::clone(&state)).await;
    let initial = coordinator.current();

    assert!(Arc::ptr_eq(&state, initial.condition()));
    assert_eq!(
        eligible_ids(initial.condition().evaluation()),
        eligible_ids(&direct)
    );

    let transition = coordinator.begin().await;
    let committed = transition
        .publish(prepared_with_condition(Arc::clone(&state)).await)
        .expect("current transition publishes");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        committed.effective_graph().generation(),
        RuntimeGenerationId::new(1)
    );
    assert!(Arc::ptr_eq(&state, committed.condition()));
}

#[tokio::test]
async fn stale_prepare_commit_rejects_without_running_the_compatibility_callback() {
    let coordinator = coordinator().await;
    let first = coordinator.begin().await;
    let committed = first
        .publish(prepared().await)
        .expect("first candidate publishes");
    let second = coordinator.begin().await;

    let callback_ran = Arc::new(AtomicBool::new(false));

    let error = second
        .prepare_commit(prepared().await)
        .map(|commit| {
            let callback_ran = Arc::clone(&callback_ran);
            commit.commit_with(move || callback_ran.store(true, Ordering::SeqCst))
        })
        .expect_err("candidate prepared from generation zero is stale");

    assert_eq!(
        error,
        StaleRuntimeProposal {
            attempt: TransitionAttemptId(2),
            base: RuntimeGenerationId::INITIAL,
            current: committed.id(),
        }
    );
    assert!(
        !callback_ran.load(Ordering::SeqCst),
        "a rejected candidate must never reach the compatibility callback"
    );
    assert_eq!(coordinator.current().id(), committed.id());

    let third = coordinator.begin().await;

    assert_eq!(third.attempt(), TransitionAttemptId(3));
    assert_eq!(third.base().id(), committed.id());
}

#[tokio::test]
async fn commit_with_publishes_before_the_compat_callback_and_holds_the_writer_through_it() {
    let coordinator = coordinator().await;
    let transition = coordinator.begin().await;
    let commit = transition
        .prepare_commit(prepared().await)
        .expect("current candidate prepares");

    let callback_entered = Arc::new(tokio::sync::Notify::new());
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let callback_calls = Arc::new(AtomicU64::new(0));
    let observed_current = Arc::new(AtomicU64::new(u64::MAX));

    let handle = {
        let callback_entered = Arc::clone(&callback_entered);
        let callback_calls = Arc::clone(&callback_calls);
        let observed_current = Arc::clone(&observed_current);
        let coordinator = coordinator.clone();

        tokio::task::spawn_blocking(move || {
            commit.commit_with(move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
                observed_current.store(coordinator.current().id().get(), Ordering::SeqCst);
                callback_entered.notify_one();

                let _ = release_rx.recv();
            })
        })
    };

    callback_entered.notified().await;

    assert_eq!(callback_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        observed_current.load(Ordering::SeqCst),
        RuntimeGenerationId::new(1).get(),
        "the candidate generation must already be current when the callback runs"
    );

    // Deterministic writer-lease check: the callback is blocked above, so the commit
    // token still owns the sole-writer guard and no contender can acquire the mutex.
    assert!(
        Arc::clone(&coordinator.writer).try_lock_owned().is_err(),
        "the writer mutex must stay locked while the compatibility callback runs"
    );

    release_tx.send(()).expect("commit task is still blocked");

    let committed = handle.await.expect("commit task does not panic");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        committed.effective_graph().generation(),
        RuntimeGenerationId::new(1)
    );
    assert!(Arc::ptr_eq(
        &committed.generation,
        &coordinator.current().generation
    ));
    assert_eq!(callback_calls.load(Ordering::SeqCst), 1);

    let second = coordinator.begin().await;

    assert_eq!(second.attempt(), TransitionAttemptId(2));
    assert_eq!(second.base().id(), committed.id());
}

// ---------------------------------------------------------------------------
// Generation-local hook-manager routing.
// ---------------------------------------------------------------------------
