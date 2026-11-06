//! Registering jobs and launching their loops.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tracing::info;

use super::{JobHandle, JobScheduler};
use crate::descriptor::{JOBS, JobOutcome};
use crate::registry::{JobEntry, JobId, JobMetadata};
use crate::run::{Runner, drive_job};
use crate::schedule::{JobOptions, Schedule, ScheduleError};
use crate::target::JobTarget;
use upwell_di::context::TargetRef;
impl JobScheduler {
    /// Schedules a job to run on `schedule`, invoking `run` on each occurrence. Returns a
    /// [`JobHandle`] to cancel it. The job is named `"dynamic"`; prefer
    /// [`schedule_named`](Self::schedule_named) when an application schedules more than one.
    ///
    /// The closure is run fresh each occurrence, so it may capture live dependencies. Runs
    /// immediately — no startup phase — so it is the right entry point for jobs discovered at
    /// run time (e.g. loaded from a database).
    pub fn schedule<F, Fut>(&self, schedule: Schedule, run: F) -> JobHandle
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JobOutcome> + Send + 'static,
    {
        self.schedule_named("dynamic", schedule, run)
    }

    /// Schedules a named dynamic job. The name flows into logs, run spans, and introspection,
    /// so distinct runtime jobs stay distinguishable.
    pub fn schedule_named<F, Fut>(
        &self,
        name: impl Into<Arc<str>>,
        schedule: Schedule,
        run: F,
    ) -> JobHandle
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JobOutcome> + Send + 'static,
    {
        self.schedule_with(
            JobMetadata::named(name.into()),
            schedule,
            JobOptions::default(),
            run,
        )
    }

    /// Schedules a dynamic job with full [`JobMetadata`] (labels, description) and
    /// [`JobOptions`] (overlap policy, timeout, jitter, …).
    pub fn schedule_with<F, Fut>(
        &self,
        metadata: JobMetadata,
        schedule: Schedule,
        options: JobOptions,
        run: F,
    ) -> JobHandle
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JobOutcome> + Send + 'static,
    {
        let run = Arc::new(run);
        let runner: Runner = Arc::new(move |_cx| {
            let run = Arc::clone(&run);

            Box::pin(async move { run().await })
        });

        self.spawn(metadata, schedule, options, runner)
    }

    /// Parses every static `#[job]`'s schedule and spawns its loop. An unparseable schedule
    /// aborts startup, so a misconfigured `#[job]` fails loudly rather than never running.
    pub(super) async fn start(&self) -> Result<(), ScheduleError> {
        let mut count = 0;

        for job in JOBS.iter() {
            let schedule = Schedule::parse(job.kind, job.schedule)?;
            let options = job.options.clone();
            let root = self.root.clone();
            let call = job.call;
            let runner: Runner = Arc::new(move |cx| call(root.clone(), cx));

            self.spawn(
                JobMetadata::named(job.name.into()),
                schedule,
                options,
                runner,
            );

            count += 1;

            info!(
                target: "upwell::jobs",
                job = job.name,
                schedule = job.schedule,
                "job scheduled"
            );
        }

        info!(target: "upwell::jobs", count, "job scheduler started");

        Ok(())
    }

    /// `runner` with each run in an invocation context of its own, holding the root for the run.
    ///
    /// A root that was never attached or is already gone has nothing to establish, so the run
    /// goes ahead without a context, and any lookup it makes fails as outside every context.
    fn in_run_context(&self, job: Arc<str>, runner: Runner) -> Runner {
        let root = self.root.clone();
        let target = TargetRef::shared(Arc::new(JobTarget { name: job }));

        Arc::new(move |cx| {
            let runner = Arc::clone(&runner);
            let context = root.context(target.clone());

            Box::pin(async move {
                let run = async move { runner(cx).await };

                match context {
                    Some(context) => context.enter(run).await,
                    None => run.await,
                }
            })
        })
    }

    /// The shared spawn path: registers an entry, launches its loop, and arranges the job to
    /// remove itself from the registry when its loop ends.
    fn spawn(
        &self,
        metadata: JobMetadata,
        schedule: Schedule,
        options: JobOptions,
        runner: Runner,
    ) -> JobHandle {
        let id = JobId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let token = self.cancel.child_token();
        let runner = self.in_run_context(Arc::clone(&metadata.name), runner);

        let entry = Arc::new(JobEntry::new(
            id,
            metadata,
            token,
            runner,
            Arc::clone(&self.run_ids),
            Arc::new(schedule),
            options,
        ));

        self.registry.insert(Arc::clone(&entry));

        let registry = Arc::clone(&self.registry);
        let loop_entry = Arc::clone(&entry);

        tokio::spawn(drive_job(loop_entry, move || registry.remove(id)));

        JobHandle { entry }
    }
}
