//! Carrying the context into rayon pools.

use ::rayon::prelude::*;

use super::super::rayon as context_rayon;
use super::super::rayon::ParallelIteratorExt;
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

    let (left, right) = captured.run(|| {
        context_rayon::install(&pool, || {
            context_rayon::join(|| sees(&request), || sees(&request))
        })
    });

    assert!(left && right);
    assert!(pool.install(sees_nothing), "an unbound job sees nothing");
}

#[tokio::test]
async fn a_rayon_spawn_carries_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let (sender, receiver) = std::sync::mpsc::channel();

    captured.run(|| {
        context_rayon::spawn(move || {
            sender
                .send(sees(&request))
                .expect("the test waits for the job");
        });
    });

    assert!(receiver.recv().expect("the job reports"));
}

#[tokio::test]
async fn a_rayon_scope_and_its_nested_jobs_carry_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(2);
    let (sender, receiver) = std::sync::mpsc::channel();
    let report = &sender;
    let request = &request;

    let body = captured.run(|| {
        context_rayon::install(&pool, || {
            context_rayon::scope(|scope| {
                scope.spawn(move |nested| {
                    nested.spawn(move |_| {
                        report.send(sees(request)).expect("the test waits");
                    });
                    report.send(sees(request)).expect("the test waits");
                });

                sees(request)
            })
        })
    });

    drop(sender);

    assert!(body, "the scope body runs in the context");
    assert_eq!(receiver.iter().collect::<Vec<_>>(), [true, true]);
}

#[tokio::test]
async fn a_current_iterator_carries_the_context_onto_every_worker() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(2);

    let iter = captured.clone().run(|| (0..64).into_par_iter().current());
    let seen = pool.install(|| iter.map(|_| sees(&request)).all(|seen| seen));
    let reduced = captured.run(|| {
        (0..64).into_par_iter().current().map(|_| 1_usize).reduce(
            || 0,
            |left, right| left + right * usize::from(sees(&request)),
        )
    });

    assert!(seen, "every item is mapped inside the context");
    assert_eq!(reduced, 64, "every reduction runs inside the context");
}

#[tokio::test]
async fn a_current_iterator_initializes_its_folds_inside_the_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(2);

    let folds = captured.clone().run(|| (0..64).into_par_iter().current());
    let reductions = captured.run(|| (0..64).into_par_iter().current());
    let folded = pool.install(|| {
        folds
            .fold(|| sees(&request), |seen, _| seen)
            .all(|seen| seen)
    });
    let reduced = pool.install(|| {
        reductions
            .map(|_| true)
            .reduce(|| sees(&request), |left, right| left && right)
    });

    assert!(folded, "every fold identity runs inside the context");
    assert!(reduced, "every reduce identity runs inside the context");
}

#[tokio::test]
async fn pool_jobs_never_see_each_others_contexts() {
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
async fn a_job_captured_outside_every_context_never_sees_a_waiting_workers_context() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(1);
    let (sender, receiver) = std::sync::mpsc::channel();
    let unbound = CapturedContext::active().bind(move || {
        sender.send(sees_nothing()).expect("the test waits");
    });

    let yielded = captured.run(|| {
        context_rayon::install(&pool, || {
            ::rayon::spawn(unbound);

            let yielded = ::rayon::yield_now();

            assert!(sees(&request), "the waiting job keeps its own context");

            yielded
        })
    });

    assert_eq!(
        yielded,
        Some(::rayon::Yield::Executed),
        "the worker ran the job while it waited"
    );
    assert!(
        receiver.recv().expect("the job reports"),
        "the context-free job ran without the waiting job's context"
    );
}

#[tokio::test]
async fn nested_parallel_work_in_a_current_closure_carries_the_context_again() {
    let chain = chain().await;
    let (captured, request) = captured(&chain).await;
    let pool = pool(2);

    let outer = captured.run(|| (0..8).into_par_iter().current());
    let seen = pool.install(|| {
        outer
            .map(|_| {
                (0..512)
                    .into_par_iter()
                    .current()
                    .map(|_| sees(&request))
                    .all(|seen| seen)
            })
            .all(|seen| seen)
    });

    assert!(seen, "every stolen inner job runs inside the context");
}
