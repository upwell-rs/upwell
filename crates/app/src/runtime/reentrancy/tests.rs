use super::*;

fn active(owner: &Arc<()>, attempt: u64) -> ActiveTransition {
    ActiveTransition::new(Arc::clone(owner), TransitionAttemptId::new(attempt))
}

#[tokio::test]
async fn no_transition_is_active_outside_a_scope() {
    let owner = Arc::new(());

    assert_eq!(ActiveTransition::current_for(&owner), None);
}

#[tokio::test]
async fn the_scope_marks_its_own_runtime_with_the_running_attempt() {
    let owner = Arc::new(());

    let inside = active(&owner, 4)
        .scope(async { ActiveTransition::current_for(&owner) })
        .await;

    assert_eq!(inside, Some(TransitionAttemptId::new(4)));
    assert_eq!(
        ActiveTransition::current_for(&owner),
        None,
        "the marker ends with its scope"
    );
}

#[tokio::test]
async fn another_runtimes_transition_is_not_reentrant() {
    let running = Arc::new(());
    let other = Arc::new(());

    let inside = active(&running, 4)
        .scope(async { ActiveTransition::current_for(&other) })
        .await;

    assert_eq!(inside, None, "a different runtime's leases are not held");
}

#[tokio::test]
async fn an_outer_runtime_stays_marked_inside_a_nested_runtimes_transition() {
    let outer = Arc::new(());
    let inner = Arc::new(());

    let (outer_seen, inner_seen) = active(&outer, 4)
        .scope(active(&inner, 9).scope(async {
            (
                ActiveTransition::current_for(&outer),
                ActiveTransition::current_for(&inner),
            )
        }))
        .await;

    assert_eq!(
        outer_seen,
        Some(TransitionAttemptId::new(4)),
        "an A -> B -> A reload cycle is still rejected for A"
    );
    assert_eq!(inner_seen, Some(TransitionAttemptId::new(9)));
}

#[tokio::test]
async fn a_nested_scope_ends_without_unmarking_the_outer_transition() {
    let outer = Arc::new(());
    let inner = Arc::new(());

    let (after_inner, inner_after) = active(&outer, 4)
        .scope(async {
            active(&inner, 9).scope(async {}).await;

            (
                ActiveTransition::current_for(&outer),
                ActiveTransition::current_for(&inner),
            )
        })
        .await;

    assert_eq!(after_inner, Some(TransitionAttemptId::new(4)));
    assert_eq!(inner_after, None, "the inner marker ends with its scope");
}

#[tokio::test]
async fn spawned_work_does_not_inherit_the_marker() {
    let owner = Arc::new(());
    let spawned_owner = Arc::clone(&owner);

    let spawned = active(&owner, 4)
        .scope(async move {
            tokio::spawn(async move { ActiveTransition::current_for(&spawned_owner) }).await
        })
        .await
        .expect("the spawned task completes");

    assert_eq!(spawned, None, "spawned work waits for the lease instead");
}
