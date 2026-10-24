use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::sync::mpsc::error::TryRecvError;
use upwell_di::{Component, RootResolver};
use upwell_hooks::{HookKind, Startup};

use super::{JobScheduler, scheduler_descriptor};
use crate::registry::{JobState, JobTrigger};
use crate::schedule::Schedule;

async fn wait_for_idle(handle: &super::JobHandle) {
    while handle.entry.active() != 0 {
        tokio::task::yield_now().await;
    }
}

#[test]
fn descriptor_identity_matches_component() {
    let descriptor = scheduler_descriptor();

    assert_eq!(descriptor.id, <JobScheduler as Component>::ID);
    assert_eq!(descriptor.name, <JobScheduler as Component>::NAME);
    assert_eq!(descriptor.scope.name(), "Singleton");
}

#[test]
fn descriptor_carries_one_startup_hook() {
    let descriptor = scheduler_descriptor();
    let hooks = (descriptor.hooks)();

    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].kind, <Startup as HookKind>::NAME);
}

#[test]
fn descriptor_carries_a_non_default_factory() {
    let descriptor = scheduler_descriptor();
    let factories = (descriptor.factories)();

    assert_eq!(factories.len(), 1);
    assert_eq!(factories[0].role, upwell_di::ComponentFactoryRole::Explicit);
}

/// The scheduler needs no live container for dynamic jobs — the runner closure captures its
/// own state — so an unattached `RootResolver` is enough to exercise the runtime path.
#[tokio::test(start_paused = true)]
async fn dynamic_job_runs_then_cancels() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (run_tx, mut run_rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = scheduler.schedule(Schedule::every(Duration::from_millis(10)), move || {
        let run_tx = run_tx.clone();

        async move {
            run_tx.send(()).expect("test is still observing runs");

            Ok(())
        }
    });

    tokio::task::yield_now().await;

    for expected in 1..=2 {
        tokio::time::advance(Duration::from_millis(10)).await;
        run_rx
            .recv()
            .await
            .unwrap_or_else(|| panic!("job runner channel closed before run {expected}"));
    }

    scheduler
        .cancel_and_wait(handle.id())
        .await
        .expect("job exists");
    assert!(
        scheduler.registry.is_empty(),
        "cancelled job was not removed from the registry"
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_scheduler_cancels_all_jobs() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (run_tx, mut run_rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = scheduler.schedule(Schedule::every(Duration::from_millis(10)), move || {
        let run_tx = run_tx.clone();

        async move {
            run_tx.send(()).expect("test is still observing runs");

            Ok(())
        }
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    run_rx.recv().await.expect("job runner channel stays open");

    drop(scheduler);
    handle.entry.wait_done().await;

    assert!(handle.is_cancelled());

    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;

    assert_eq!(run_rx.try_recv(), Err(TryRecvError::Empty));
}

/// A counting dynamic job on a long interval, so only explicit triggers run it during a test.
fn counting_job(scheduler: &JobScheduler, name: &str, runs: Arc<AtomicUsize>) -> super::JobHandle {
    scheduler.schedule_named(
        name,
        Schedule::every(Duration::from_secs(3600)),
        move || {
            let runs = Arc::clone(&runs);

            async move {
                runs.fetch_add(1, Ordering::Relaxed);

                Ok(())
            }
        },
    )
}

#[tokio::test]
async fn run_now_triggers_a_manual_run() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (run_tx, mut run_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = scheduler.schedule_named(
        "Manual::job",
        Schedule::every(Duration::from_secs(3600)),
        move || {
            let run_tx = run_tx.clone();

            async move {
                run_tx.send(()).expect("test is still observing runs");

                Ok(())
            }
        },
    );

    let run_id = scheduler.run_now(handle.id()).await.expect("job exists");

    run_rx.recv().await.expect("manual run did not fire");
    assert_eq!(run_rx.try_recv(), Err(TryRecvError::Empty));

    let recent = scheduler.recent_runs(handle.id());

    assert!(
        recent
            .iter()
            .any(|r| r.run_id == run_id && r.trigger == JobTrigger::Manual)
    );
}

#[tokio::test]
async fn run_now_on_unknown_job_errors() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let handle = counting_job(&scheduler, "Known::job", Arc::new(AtomicUsize::new(0)));
    handle.cancel();
    scheduler
        .cancel_and_wait(handle.id())
        .await
        .expect("job exists");

    assert!(scheduler.run_now(handle.id()).await.is_err());
}

#[tokio::test]
async fn list_jobs_and_job_reflect_registration() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let handle = counting_job(&scheduler, "Listed::job", Arc::new(AtomicUsize::new(0)));

    assert_eq!(scheduler.list_jobs().len(), 1);

    let info = scheduler.job(handle.id()).expect("job listed");

    assert_eq!(&*info.name, "Listed::job");
    assert_eq!(info.state, JobState::Scheduled);
}

#[tokio::test(start_paused = true)]
async fn pause_prevents_scheduled_runs() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (run_tx, mut run_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = scheduler.schedule_named(
        "Paused::job",
        Schedule::every(Duration::from_millis(15)),
        move || {
            let run_tx = run_tx.clone();

            async move {
                run_tx.send(()).expect("test is still observing runs");

                Ok(())
            }
        },
    );

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(15)).await;
    run_rx.recv().await.expect("scheduled run did not fire");
    wait_for_idle(&handle).await;

    scheduler.pause(handle.id()).expect("job exists");
    tokio::time::advance(Duration::from_millis(60)).await;
    tokio::task::yield_now().await;

    assert_eq!(run_rx.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(scheduler.job(handle.id()).unwrap().state, JobState::Paused);

    scheduler.resume(handle.id()).expect("job exists");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(15)).await;

    run_rx.recv().await.expect("job did not resume");
}

#[tokio::test]
async fn cancel_and_wait_removes_the_job() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let handle = counting_job(&scheduler, "Doomed::job", Arc::new(AtomicUsize::new(0)));

    scheduler
        .cancel_and_wait(handle.id())
        .await
        .expect("job exists");

    assert!(scheduler.job(handle.id()).is_none());
    assert!(scheduler.registry.is_empty());
}

#[tokio::test]
async fn reschedule_changes_the_reported_cadence() {
    use crate::schedule::ScheduleInfo;

    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let handle = counting_job(&scheduler, "Recadenced::job", Arc::new(AtomicUsize::new(0)));

    scheduler
        .reschedule(handle.id(), Schedule::cron("@hourly").unwrap())
        .expect("job exists");

    let info = scheduler.job(handle.id()).expect("job exists");

    assert!(matches!(info.schedule, ScheduleInfo::Cron(_)));
}

#[tokio::test(start_paused = true)]
async fn timeout_marks_a_run_timed_out() {
    use crate::registry::JobRunOutcome;
    use crate::schedule::JobOptions;

    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let options = JobOptions {
        timeout: Some(Duration::from_millis(20)),
        ..JobOptions::default()
    };
    let handle = scheduler.schedule_with(
        crate::registry::JobMetadata::named("Slow::job".into()),
        Schedule::every(Duration::from_secs(3600)),
        options,
        move || {
            let started_tx = started_tx.clone();

            async move {
                started_tx.send(()).expect("test is still observing runs");
                tokio::time::sleep(Duration::from_millis(500)).await;

                Ok(())
            }
        },
    );

    scheduler.run_now(handle.id()).await.expect("job exists");
    started_rx.recv().await.expect("manual run did not start");
    tokio::time::advance(Duration::from_millis(20)).await;

    while scheduler
        .job(handle.id())
        .is_some_and(|info| info.failure_count == 0)
    {
        tokio::task::yield_now().await;
    }

    let info = scheduler.job(handle.id()).expect("job exists");

    assert_eq!(info.last_run.unwrap().outcome, JobRunOutcome::TimedOut);
    assert_eq!(info.failure_count, 1);
}

#[tokio::test(start_paused = true)]
async fn skip_overlap_defers_while_a_run_is_active() {
    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let gate = Arc::new(Semaphore::new(0));
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle =
        scheduler.schedule_named("Skippy::job", Schedule::every(Duration::from_millis(15)), {
            let gate = Arc::clone(&gate);

            move || {
                let gate = Arc::clone(&gate);
                let started_tx = started_tx.clone();

                async move {
                    started_tx.send(()).expect("test is still observing runs");
                    let _permit = gate.acquire().await.expect("semaphore stays open");

                    Ok(())
                }
            }
        });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(15)).await;
    started_rx
        .recv()
        .await
        .expect("scheduled run did not start");
    tokio::time::advance(Duration::from_millis(15)).await;
    tokio::task::yield_now().await;

    let info = scheduler.job(handle.id()).expect("job exists");

    assert!(info.skipped_count > 0, "expected firings to be skipped");

    gate.add_permits(1);
    wait_for_idle(&handle).await;
}

#[tokio::test]
async fn queue_one_preserves_the_deferred_manual_run_identity() {
    use crate::schedule::{JobOptions, OverlapPolicy};

    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let gate = Arc::new(Semaphore::new(0));
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let options = JobOptions {
        overlap: OverlapPolicy::QueueOne,
        ..JobOptions::default()
    };
    let handle = scheduler.schedule_with(
        crate::registry::JobMetadata::named("Queued::job".into()),
        Schedule::every(Duration::from_secs(3600)),
        options,
        {
            let gate = Arc::clone(&gate);

            move || {
                let gate = Arc::clone(&gate);
                let started_tx = started_tx.clone();

                async move {
                    started_tx.send(()).expect("test is still observing runs");
                    let _permit = gate.acquire().await.expect("semaphore stays open");

                    Ok(())
                }
            }
        },
    );

    let first = scheduler.run_now(handle.id()).await.expect("job exists");
    started_rx
        .recv()
        .await
        .expect("first manual run did not start");

    let second = scheduler.run_now(handle.id()).await.expect("job exists");
    tokio::task::yield_now().await;

    assert_eq!(handle.entry.active(), 1);
    assert_eq!(started_rx.try_recv(), Err(TryRecvError::Empty));

    gate.add_permits(1);
    started_rx
        .recv()
        .await
        .expect("deferred manual run did not start");

    let recent = scheduler.recent_runs(handle.id());

    let deferred = recent
        .iter()
        .find(|r| r.run_id == second)
        .expect("deferred manual run recorded under its returned id");

    assert_eq!(deferred.trigger, JobTrigger::Manual);
    assert!(recent.iter().any(|r| r.run_id == first));

    gate.add_permits(1);
    wait_for_idle(&handle).await;
}

#[tokio::test(start_paused = true)]
async fn allow_overlap_permits_concurrent_runs() {
    use crate::schedule::{JobOptions, OverlapPolicy};

    let scheduler = JobScheduler::create(RootResolver::new()).await;
    let gate = Arc::new(Semaphore::new(0));
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let options = JobOptions {
        overlap: OverlapPolicy::Allow,
        ..JobOptions::default()
    };
    let handle = scheduler.schedule_with(
        crate::registry::JobMetadata::named("Overlapping::job".into()),
        Schedule::every(Duration::from_millis(20)),
        options,
        {
            let gate = Arc::clone(&gate);

            move || {
                let gate = Arc::clone(&gate);
                let started_tx = started_tx.clone();

                async move {
                    started_tx.send(()).expect("test is still observing runs");
                    let _permit = gate.acquire().await.expect("semaphore stays open");

                    Ok(())
                }
            }
        },
    );

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(20)).await;
    started_rx.recv().await.expect("first run did not start");
    tokio::time::advance(Duration::from_millis(20)).await;
    started_rx.recv().await.expect("second run did not start");

    assert!(
        scheduler.metrics().active_runs > 1,
        "expected overlapping runs under OverlapPolicy::Allow"
    );

    gate.add_permits(2);
    wait_for_idle(&handle).await;
}
