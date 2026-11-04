//! Carrying the context onto the blocking pool, OS threads and synchronous closures.

use super::*;
use crate::context::Current;

/// A value seeded into a request, read back synchronously.
#[derive(Debug, PartialEq, Eq)]
struct Marker(u8);

#[tokio::test]
async fn blocking_and_thread_work_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let on_blocking = Arc::clone(&request);
    let on_thread = Arc::clone(&request);

    let blocking = captured
        .clone()
        .spawn_blocking(move || sees(&on_blocking))
        .await;
    let thread = captured
        .clone()
        .spawn_thread(move || sees(&on_thread))
        .join();
    let plain = std::thread::spawn(sees_nothing).join();
    let free = captured
        .enter(async { spawn_blocking(|| innermost().is_ok()).await })
        .await;

    assert!(blocking.expect("the blocking task completes"));
    assert!(thread.expect("the thread completes"));
    assert!(plain.expect("the plain thread completes"));
    assert!(free.expect("the free blocking task completes"));
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
    let request = crate::context::fixtures::seeded::<Request>(
        &chain.root,
        vec![crate::context::fixtures::seed(Marker(9))],
    )
    .await;
    let captured = context(&chain, Invocation::HttpRequest)
        .with_scope(request)
        .enter(async { CapturedContext::capture().expect("the context is active") })
        .await;
    let handle = tokio::runtime::Handle::current();

    let marker = captured
        .spawn_blocking(move || handle.block_on(Current::<Arc<Marker>>::new().get()))
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
