//! Carrying the context onto the blocking pool, OS threads and synchronous closures.

use super::*;
use crate::context::Current;
use crate::context::fixtures::{seed, seeded};

/// A value seeded into a request, read back synchronously.
#[derive(Debug, PartialEq, Eq)]
struct Marker(u8);

#[tokio::test]
async fn blocking_and_thread_work_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let on_blocking = Arc::clone(&request);

    let (blocking, thread) = captured
        .enter(async move {
            let blocking = context_tokio::spawn_blocking(move || sees(&on_blocking)).await;
            let thread = context_thread::spawn(move || sees(&request)).join();

            (blocking, thread)
        })
        .await;
    let plain = std::thread::spawn(sees_nothing).join();

    assert!(blocking.expect("the blocking task completes"));
    assert!(thread.expect("the thread completes"));
    assert!(plain.expect("the plain thread completes"));
}

#[tokio::test]
async fn scoped_threads_carry_the_context_and_borrow_their_environment() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;

    let seen = captured.run(|| {
        context_thread::scope(|threads| {
            let first = threads.spawn(|| sees(&request));
            let second = threads.spawn(|| sees(&request));

            [first.join(), second.join()]
        })
    });
    let outside = context_thread::scope(|threads| threads.spawn(sees_nothing).join());

    assert!(
        seen.into_iter()
            .all(|seen| seen.expect("the scoped thread completes"))
    );
    assert!(outside.expect("the scoped thread completes"));
}

#[tokio::test]
async fn a_bound_closure_restores_the_threads_previous_context() {
    let chain = chain().await;
    let (outer, outer_request) = captured(&chain).await;
    let (inner, inner_request) = captured(&chain).await;

    let (during, after) = outer.run(|| {
        let during = inner.bind(|| sees(&inner_request))();

        (during, sees(&outer_request))
    });

    assert!(during, "the bound closure sees its own context");
    assert!(after, "the thread's context is back once it returns");
    assert!(sees_nothing(), "nothing stays behind on the thread");
}

#[tokio::test]
async fn sync_code_resolves_through_the_same_context_as_async_code() {
    let chain = chain().await;
    let request = seeded::<Request>(&chain.root, vec![seed(Marker(9))]).await;
    let entered = context(&chain, &Entry::Http).with_scope(request);
    let handle = tokio::runtime::Handle::current();

    let marker = entered
        .enter(async move {
            context_tokio::spawn_blocking(move || {
                handle.block_on(Current::<Arc<Marker>>::new().get())
            })
            .await
        })
        .await
        .expect("the blocking task completes");

    assert_eq!(*marker.expect("the seeded marker"), Marker(9));
}

#[test]
fn a_bound_fn_enters_the_context_on_every_call() {
    let runtime = tokio::runtime::Runtime::new().expect("a runtime starts");
    let chain = runtime.block_on(chain());
    let (captured, request) = runtime.block_on(captured(&chain));
    let check = captured.bind_fn(|count: usize| (0..count).all(|_| sees(&request)));

    let seen = std::thread::scope(|threads| {
        let first = threads.spawn(|| check(3));
        let second = threads.spawn(|| check(5));

        [first.join(), second.join()]
    });

    assert!(
        seen.into_iter()
            .all(|seen| seen.expect("the thread completes"))
    );
}
