//! The invocation context each job run executes in.

use std::collections::HashMap;

use tokio::sync::mpsc;
use upwell_core::ResolverSet;
use upwell_di::context::{ContextError, InvocationContext};
use upwell_di::{ScopeContainer, ScopeRegistry};

use super::*;
use crate::target::JobTarget;

/// A root, and a resolver attached to it.
async fn attached() -> (Arc<ScopeContainer>, RootResolver) {
    let registry = ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
        .expect("an empty scope registry validates");
    let root = ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::new(registry))
        .await
        .expect("the root builds");
    let resolver = RootResolver::new();

    resolver.attach(&root);

    (root, resolver)
}

/// What one run of the job named `name` saw of its context.
#[derive(Debug)]
struct Run {
    name: &'static str,
    target: Option<JobTarget>,
    resolves_root: bool,
    has_scope: bool,
}

/// Schedules a job reporting its context after yielding, so runs of both jobs interleave.
fn report_runs(
    scheduler: &JobScheduler,
    root: &Arc<ScopeContainer>,
    name: &'static str,
    reports: &mpsc::UnboundedSender<Run>,
) -> crate::scheduler::JobHandle {
    let root = Arc::clone(root);
    let reports = reports.clone();

    scheduler.schedule_named(name, Schedule::every(Duration::from_millis(1)), move || {
        let root = Arc::clone(&root);
        let reports = reports.clone();

        async move {
            tokio::task::yield_now().await;

            let context = InvocationContext::capture().expect("a job run has a context");
            let run = Run {
                name,
                target: context.target().downcast_ref::<JobTarget>().cloned(),
                resolves_root: context.resolves_root(&root),
                has_scope: context.scope_key().is_some(),
            };

            let _ = reports.send(run);

            Ok(())
        }
    })
}

#[tokio::test(start_paused = true)]
async fn concurrent_job_runs_each_see_their_own_job_and_the_root() {
    let (root, resolver) = attached().await;
    let scheduler = JobScheduler::create(resolver).await;
    let (reports, mut runs) = mpsc::unbounded_channel();
    let first = report_runs(&scheduler, &root, "first", &reports);
    let second = report_runs(&scheduler, &root, "second", &reports);

    tokio::task::yield_now().await;

    for _ in 0..8 {
        tokio::time::advance(Duration::from_millis(1)).await;

        let run = runs.recv().await.expect("the jobs keep running");

        assert_eq!(
            run.target,
            Some(JobTarget {
                name: Arc::from(run.name)
            })
        );
        assert!(run.resolves_root);
        assert!(!run.has_scope, "a job run opens no scope");
    }

    first.cancel();
    second.cancel();
}

#[tokio::test(start_paused = true)]
async fn a_scheduler_without_a_root_runs_jobs_without_a_context() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (reports, mut errors) = mpsc::unbounded_channel();
    let handle = scheduler.schedule(Schedule::every(Duration::from_millis(1)), move || {
        let reports = reports.clone();

        async move {
            let _ = reports.send(InvocationContext::capture().err());

            Ok(())
        }
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1)).await;

    assert_eq!(
        errors.recv().await.expect("the job runs"),
        Some(ContextError::OutsideContext)
    );

    handle.cancel();
}
