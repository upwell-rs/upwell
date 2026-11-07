use std::sync::Arc;

use upwell_core::{ResolutionMode, Scope};

use super::*;
use crate::context::ContextError;
use crate::context::fixtures::{Entry, Message, Request, chain, child, context, seed, seeded};

/// The identity an invocation authenticated as.
#[derive(Debug, PartialEq, Eq)]
struct Identity(u32);

/// A value nothing seeds or registers.
struct Unprovided;

/// A singleton-held accessor, shared by every invocation.
static IDENTITY: Current<Arc<Identity>> = Current::new();

async fn read<T: FromContainer>(accessor: Current<T>) -> Result<T, ScopedError> {
    accessor.get().await
}

#[tokio::test]
async fn one_accessor_resolves_each_concurrent_invocations_own_value() {
    let chain = chain().await;
    let mut tasks = tokio::task::JoinSet::new();

    for id in 0..8 {
        let request = seeded::<Request>(&chain.root, vec![seed(Identity(id))]).await;
        let entered = context(&chain, &Entry::Http).with_scope(request);

        tasks.spawn(entered.enter(async move {
            for _ in 0..4 {
                tokio::task::yield_now().await;

                assert_eq!(
                    *IDENTITY.get().await.expect("the request's identity"),
                    Identity(id)
                );
            }
        }));
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("every invocation sees only its own identity");
    }
}

#[tokio::test]
async fn current_prefers_the_closest_instance_and_scoped_pins_its_scope() {
    let chain = chain().await;
    let connection = seeded::<Request>(&chain.root, vec![seed(Identity(1))]).await;
    let message = seeded::<Message>(&connection, vec![seed(Identity(2))]).await;
    let entered = context(&chain, &Entry::Message).with_scope(message);

    entered
        .enter(async {
            let closest = IDENTITY.get().await.expect("the message identity");
            let pinned = Scoped::<Request, Arc<Identity>>::new()
                .get()
                .await
                .expect("the connection identity");
            let own = Scoped::<Message, Arc<Identity>>::new()
                .get()
                .await
                .expect("the message identity");

            assert_eq!(*closest, Identity(2));
            assert_eq!(*pinned, Identity(1));
            assert_eq!(*own, Identity(2));
        })
        .await;
}

#[tokio::test]
async fn a_value_held_by_a_parent_resolves_through_the_chain() {
    let chain = chain().await;
    let connection = seeded::<Request>(&chain.root, vec![seed(Identity(7))]).await;
    let message = child::<Message>(&connection).await;
    let entered = context(&chain, &Entry::Message).with_scope(message);

    let identity = entered.enter(read(IDENTITY)).await;

    assert_eq!(*identity.expect("the connection identity"), Identity(7));
}

#[tokio::test]
async fn a_pinned_scope_missing_from_the_chain_is_its_own_error() {
    let chain = chain().await;
    let entered = context(&chain, &Entry::Http).with_scope(Arc::clone(&chain.request));

    let result = entered
        .enter(Scoped::<Message, Arc<Identity>>::new().get())
        .await;

    assert!(matches!(
        result,
        Err(ScopedError::ScopeNotActive { scope, name })
            if scope == Message::ID && name == Message::NAME
    ));
}

#[tokio::test]
async fn an_unprovided_target_is_unavailable() {
    let chain = chain().await;
    let entered = context(&chain, &Entry::Http).with_scope(Arc::clone(&chain.request));

    let result = entered.enter(read(Current::<Arc<Unprovided>>::new())).await;

    assert!(matches!(result, Err(ScopedError::Unavailable { .. })));
}

#[tokio::test]
async fn accessors_fail_with_typed_errors_outside_an_invocation_scope() {
    let chain = chain().await;
    let hook = context(&chain, &Entry::Hook);

    let outside = IDENTITY.get().await;
    let scopeless = hook.enter(read(IDENTITY)).await;

    assert!(matches!(
        outside,
        Err(ScopedError::Context(ContextError::OutsideContext))
    ));
    assert!(matches!(
        scopeless,
        Err(ScopedError::Context(ContextError::OutsideInvocationScope))
    ));
}

#[tokio::test]
async fn nothing_stays_reachable_once_the_invocation_ends() {
    let chain = chain().await;
    let request = seeded::<Request>(&chain.root, vec![seed(Identity(3))]).await;
    let weak = Arc::downgrade(&request);
    let entered = context(&chain, &Entry::Http).with_scope(request);

    let during = entered.enter(read(IDENTITY)).await;

    assert_eq!(
        *during.expect("the identity during the request"),
        Identity(3)
    );
    assert!(weak.upgrade().is_none(), "the accessor kept no container");
    assert!(matches!(
        IDENTITY.get().await,
        Err(ScopedError::Context(ContextError::OutsideContext))
    ));
}

#[test]
fn accessors_are_zero_sized_and_declare_an_invocation_context_edge() {
    let current = <Current<Arc<Identity>> as FromContainer>::dependency();
    let scoped = <Scoped<Request, Arc<Identity>> as FromContainer>::dependency();
    let target = <Arc<Identity> as FromContainer>::dependency();

    assert_eq!(size_of::<Current<Arc<Identity>>>(), 0);
    assert_eq!(size_of::<Scoped<Request, Arc<Identity>>>(), 0);

    for edge in [current, scoped] {
        assert_eq!(edge.resolution, ResolutionMode::Current);
        assert_eq!(edge.ty.type_id, target.ty.type_id);
        assert_eq!(edge.cardinality, target.cardinality);
        assert!(!edge.optional && !edge.dynamic && !edge.config);
    }
}

#[test]
fn a_missing_pinned_scope_names_its_id_and_name() {
    let message = ScopedError::scope_not_active::<Message>().to_string();

    assert!(message.contains(Message::ID.as_str()), "{message}");
    assert!(message.contains(Message::NAME), "{message}");
}

#[tokio::test]
async fn a_scope_the_invocation_never_opened_is_reported_as_not_active() {
    let chain = chain().await;
    let under_root = context(&chain, &Entry::Rpc)
        .with_pending_scope(Request::KEY, Some(Arc::clone(&chain.root)));
    let without_parent = context(&chain, &Entry::Http).with_pending_scope(Request::KEY, None);

    let missing = under_root.enter(read(IDENTITY)).await;
    let scopeless = without_parent.enter(read(IDENTITY)).await;

    for result in [missing, scopeless] {
        assert!(matches!(
            result,
            Err(ScopedError::ScopeNotActive { scope, name })
                if scope == Request::ID && name == Request::NAME
        ));
    }
}
