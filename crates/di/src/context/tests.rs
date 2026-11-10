use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use upwell_core::Scope;

use super::fixtures::{Custom, Entry, Message, Request, chain, child, context, innermost, runtime};
use super::{
    ContextError, ContextState, InvocationContext, TargetRef, bind_opened_scope, with_context_state,
};
use crate::test_support;

#[tokio::test]
async fn lookups_outside_every_context_fail_with_a_typed_error() {
    let chain = chain().await;

    assert_eq!(
        InvocationContext::capture()
            .err()
            .map(|error| error.to_string()),
        Some(ContextError::OutsideContext.to_string())
    );
    assert_eq!(innermost().err(), Some(ContextError::OutsideContext));
    assert!(!bind_opened_scope(&chain.request));
}

#[tokio::test]
async fn an_entered_context_stays_visible_across_awaits() {
    let chain = chain().await;
    let entered = context(&chain, &Entry::Http).with_scope(Arc::clone(&chain.request));

    entered
        .enter(async {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;

            let current = InvocationContext::capture().expect("the context is active");

            assert_eq!(current.target().downcast_ref::<Entry>(), Some(&Entry::Http));
            assert!(current.is_scope(&chain.request));
            assert_eq!(current.scope_key(), Some(Request::KEY));
            assert!(current.resolves_root(&chain.root));
        })
        .await;
}

#[tokio::test]
async fn a_nested_context_shadows_the_outer_one_until_it_ends() {
    let chain = chain().await;
    let message = child::<Message>(&chain.request).await;
    let outer = context(&chain, &Entry::Connection).with_scope(Arc::clone(&chain.request));
    let inner = context(&chain, &Entry::Message).with_scope(Arc::clone(&message));

    outer
        .enter(async {
            inner
                .enter(async {
                    let current = InvocationContext::capture().expect("inner context");

                    assert_eq!(
                        current.target().downcast_ref::<Entry>(),
                        Some(&Entry::Message)
                    );
                    assert!(current.is_scope(&message));
                })
                .await;

            let current = InvocationContext::capture().expect("outer context");

            assert_eq!(
                current.target().downcast_ref::<Entry>(),
                Some(&Entry::Connection)
            );
            assert!(current.is_scope(&chain.request));
        })
        .await;
}

#[tokio::test]
async fn a_context_without_a_scope_resolves_the_runtime_root_only() {
    let chain = chain().await;
    let hook = context(&chain, &Entry::Hook);

    hook.enter(async {
        let current = InvocationContext::capture().expect("hook context");

        assert_eq!(
            innermost().err(),
            Some(ContextError::OutsideInvocationScope)
        );
        assert_eq!(current.scope_key(), None);
        assert!(current.resolves_root(&chain.root));
    })
    .await;
}

#[tokio::test]
async fn a_context_in_a_scope_resolves_the_root_its_chain_pinned() {
    let pinned = chain().await;
    let current = chain().await;
    let entered = context(&current, &Entry::Rpc).with_scope(Arc::clone(&pinned.request));

    assert!(entered.resolves_root(&pinned.root));
    assert!(!entered.resolves_root(&current.root));
}

#[tokio::test]
async fn a_pending_scope_shows_its_parent_until_its_own_instance_binds() {
    let chain = chain().await;
    let message = child::<Message>(&chain.request).await;
    let stray = child::<Message>(&chain.request).await;
    let other_parent = child::<Request>(&chain.root).await;
    let foreign = child::<Message>(&other_parent).await;
    let pending = context(&chain, &Entry::Rpc)
        .with_pending_scope(Message::KEY, Some(Arc::clone(&chain.request)));

    pending
        .enter(async {
            let before = InvocationContext::capture().expect("pending context");

            assert!(Arc::ptr_eq(&innermost().expect("parent"), &chain.request));
            assert!(
                !bind_opened_scope(&chain.request),
                "another scope never binds"
            );
            assert!(!bind_opened_scope(&foreign), "another parent never binds");
            assert!(bind_opened_scope(&message));
            assert!(!bind_opened_scope(&stray), "a bound scope stays bound");
            assert!(Arc::ptr_eq(&innermost().expect("opened"), &message));
            assert!(before.is_scope(&chain.request), "a capture keeps its state");
            assert!(
                InvocationContext::capture()
                    .expect("context")
                    .is_scope(&message)
            );
        })
        .await;
}

#[tokio::test]
async fn a_pending_root_level_scope_has_no_scope_until_it_opens() {
    let chain = chain().await;
    let pending = context(&chain, &Entry::Http).with_pending_scope(Request::KEY, None);

    pending
        .enter(async {
            assert_eq!(
                innermost().err(),
                Some(ContextError::OutsideInvocationScope)
            );
            assert!(bind_opened_scope(&chain.request));
            assert!(Arc::ptr_eq(&innermost().expect("opened"), &chain.request));
        })
        .await;
}

#[tokio::test]
async fn contexts_compare_by_their_innermost_scope_instance() {
    let chain = chain().await;
    let other = child::<Request>(&chain.root).await;
    let first = context(&chain, &Entry::Rpc).with_scope(Arc::clone(&chain.request));
    let second = context(&chain, &Entry::Rpc).with_scope(Arc::clone(&chain.request));
    let third = context(&chain, &Entry::Rpc).with_scope(other);
    let scopeless = context(&chain, &Entry::Rpc);

    assert!(first.same_scope(&second));
    assert!(!first.same_scope(&third));
    assert!(!first.same_scope(&scopeless));
    assert!(!scopeless.same_scope(&scopeless));
}

#[tokio::test]
async fn a_captured_context_reenters_on_another_task_and_plain_spawns_carry_nothing() {
    let chain = chain().await;
    let entered = context(&chain, &Entry::Rpc).with_scope(Arc::clone(&chain.request));

    let (captured, plain) = entered
        .enter(async {
            let captured = InvocationContext::capture().expect("context");
            let captured = tokio::spawn(captured.enter(async { innermost() }));
            let plain = tokio::spawn(async { innermost() });

            (captured.await, plain.await)
        })
        .await;

    assert!(Arc::ptr_eq(
        &captured.expect("task").expect("captured scope"),
        &chain.request
    ));
    assert_eq!(
        plain.expect("task").err(),
        Some(ContextError::OutsideContext)
    );
}

#[tokio::test]
async fn a_captured_context_reenters_synchronously_on_another_thread() {
    let chain = chain().await;
    let captured = context(&chain, &Entry::Rpc)
        .with_scope(Arc::clone(&chain.request))
        .enter(async { InvocationContext::capture().expect("context") })
        .await;
    let request = Arc::clone(&chain.request);

    let (inside, after) = std::thread::spawn(move || {
        let inside = captured.enter_sync(|| Arc::ptr_eq(&innermost().expect("scope"), &request));

        (inside, innermost().err())
    })
    .join()
    .expect("the thread completes");

    assert!(inside);
    assert_eq!(after, Some(ContextError::OutsideContext));
}

#[tokio::test]
async fn a_captured_context_holds_its_scope_instance_strongly() {
    let chain = chain().await;
    let request = child::<Request>(&chain.root).await;
    let weak = Arc::downgrade(&request);
    let captured = context(&chain, &Entry::Http)
        .with_scope(request)
        .enter(async { InvocationContext::capture().expect("context") })
        .await;

    assert!(weak.upgrade().is_some(), "the capture keeps the instance");

    drop(captured);

    assert!(
        weak.upgrade().is_none(),
        "the instance drops with its last capture"
    );
}

#[test]
fn downcasting_hits_the_defined_type_and_misses_every_other() {
    let target = TargetRef::from_static(&Entry::Rpc);

    assert_eq!(target.downcast_ref::<Entry>(), Some(&Entry::Rpc));
    assert!(target.is::<Entry>());
    assert!(target.downcast_ref::<Custom>().is_none());
    assert!(!target.is::<Custom>());
}

#[tokio::test]
async fn a_plugin_defined_target_establishes_a_context() {
    let chain = chain().await;
    let target = TargetRef::shared(Arc::new(Custom { name: "plugin" }));
    let entered = InvocationContext::new(target, runtime(&chain));

    entered
        .enter(async {
            InvocationContext::with_current(|current| {
                let custom = current.target().downcast_ref::<Custom>();

                assert_eq!(custom, Some(&Custom { name: "plugin" }));
                assert!(!current.target().is::<Entry>());
            })
            .expect("the context is active");
        })
        .await;
}

#[tokio::test]
async fn static_and_shared_targets_round_trip_through_capture() {
    let chain = chain().await;
    let shared = Arc::new(Custom { name: "shared" });
    let targets = [
        TargetRef::from_static(&Entry::Hook),
        TargetRef::shared(Arc::clone(&shared)),
    ];

    for target in targets {
        let expected = format!("{target:?}");
        let captured = InvocationContext::new(target, runtime(&chain))
            .enter(async { InvocationContext::capture().expect("the context is active") })
            .await;

        captured
            .enter(async {
                InvocationContext::with_current(|current| {
                    assert_eq!(format!("{:?}", current.target()), expected);
                })
                .expect("the re-entered context is active");
            })
            .await;
    }

    assert_eq!(
        Arc::strong_count(&shared),
        1,
        "a capture holds no target past its drop"
    );
}

#[tokio::test]
async fn concurrent_tasks_never_see_each_others_scope() {
    let chain = chain().await;
    let mut tasks = tokio::task::JoinSet::new();

    for _ in 0..16 {
        let request = child::<Request>(&chain.root).await;
        let entered = context(&chain, &Entry::Http).with_scope(Arc::clone(&request));

        tasks.spawn(entered.enter(async move {
            for _ in 0..8 {
                tokio::task::yield_now().await;

                assert!(Arc::ptr_eq(&innermost().expect("own scope"), &request));
            }
        }));
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("every task sees only its own scope");
    }
}

#[tokio::test]
async fn establishing_binding_and_capturing_allocate_nothing() {
    let chain = chain().await;
    let runtime = runtime(&chain);
    let mut cx = Context::from_waker(Waker::noop());

    let (poll, delta) = test_support::measure(|| {
        let entered = InvocationContext::new(TargetRef::from_static(&Entry::Http), runtime)
            .with_pending_scope(Request::KEY, None);
        let mut future = pin!(entered.enter(async {
            bind_opened_scope(&chain.request);

            InvocationContext::capture().map(|captured| captured.is_scope(&chain.request))
        }));

        future.as_mut().poll(&mut cx)
    });

    assert!(matches!(poll, Poll::Ready(Ok(true))));
    assert_eq!(delta.allocations, 0, "a context lives inline in its future");
}

/// The open instances `with_context_state` reports, innermost first, or `None` when absent
/// and an empty list when masked.
fn state() -> Option<Option<Vec<Arc<crate::ScopeContainer>>>> {
    with_context_state(|state| match state {
        ContextState::Absent => None,
        ContextState::Masked => Some(None),
        ContextState::Entered(chain) => Some(Some(chain.map(Arc::clone).collect())),
    })
}

#[tokio::test]
async fn the_context_state_tells_absent_masked_and_entered_apart() {
    let chain = chain().await;
    let entered = context(&chain, &Entry::Http).with_scope(Arc::clone(&chain.request));

    assert!(state().is_none(), "no context was entered");

    let (inside, masked) = entered
        .enter(async {
            let inside = state();
            let masked = InvocationContext::enter_or_mask(None, async { state() }).await;

            (inside, masked)
        })
        .await;
    let inside = inside
        .flatten()
        .expect("the entered context reports its chain");

    assert!(
        matches!(masked, Some(None)),
        "a masked context reports no chain"
    );
    assert_eq!(inside.len(), 2, "the request and the root");
    assert!(Arc::ptr_eq(&inside[0], &chain.request));
    assert!(Arc::ptr_eq(&inside[1], &chain.root));
}

#[tokio::test]
async fn an_opening_scope_shows_its_parent_and_no_opening_binds_it() {
    let chain = chain().await;
    let sibling = child::<Message>(&chain.request).await;
    let opening =
        context(&chain, &Entry::Rpc).with_opening_scope(Message::KEY, Arc::clone(&chain.request));

    opening
        .enter(async {
            assert!(Arc::ptr_eq(&innermost().expect("parent"), &chain.request));
            assert!(
                !bind_opened_scope(&sibling),
                "an instance opened while the owner's constructs never takes its place"
            );
            assert!(Arc::ptr_eq(
                &innermost().expect("still the parent"),
                &chain.request
            ));
        })
        .await;
}
