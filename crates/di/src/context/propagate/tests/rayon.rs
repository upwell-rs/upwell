//! Carrying the context into rayon pools.

use super::*;

fn pool(threads: usize) -> ::rayon::ThreadPool {
    ::rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a rayon pool starts")
}

#[tokio::test]
async fn installed_and_joined_work_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(2);

    let (left, right) = captured.install(&pool, || {
        CapturedContext::capture()
            .expect("installed work runs in the context")
            .join(|| sees(&request), || sees(&request))
    });

    assert!(left && right);
    assert!(pool.install(sees_nothing), "an unbound job sees nothing");
}

#[tokio::test]
async fn pool_jobs_never_see_each_others_contexts() {
    use ::rayon::prelude::*;

    let chain = chain().await;
    let pool = pool(2);
    let mut jobs = Vec::new();

    for _ in 0..8 {
        jobs.push(captured(&chain).await);
    }

    let seen = pool.install(|| {
        jobs.par_iter()
            .map(|(captured, request)| {
                let check = captured.clone().bind_fn(|_: usize| sees(request));

                (0..16).into_par_iter().map(check).all(|seen| seen)
            })
            .all(|seen| seen)
    });

    assert!(seen, "every bound job sees only its own context");
    assert!(
        pool.broadcast(|_| sees_nothing())
            .into_iter()
            .all(|clean| clean),
        "no worker keeps a context after its jobs"
    );
}

#[tokio::test]
async fn a_rayon_spawn_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let (sender, receiver) = std::sync::mpsc::channel();

    captured.spawn_rayon(move || {
        sender
            .send(sees(&request))
            .expect("the test waits for the job");
    });

    assert!(receiver.recv().expect("the job reports"));
}
