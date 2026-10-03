//! The `JobScheduler` singleton: drives one supervised loop per job over a shared [`JobRegistry`].
//!
//! Jobs come from two places, both funnelling through the same [`spawn`](JobScheduler::spawn)
//! path:
//!
//! - **static** — every `#[job]` in the binary, discovered at link time from the [`JOBS`]
//!   slice and started by the scheduler's `Startup` hook.
//! - **dynamic** — added at run time through [`JobScheduler::schedule`] /
//!   [`schedule_named`](JobScheduler::schedule_named), for jobs whose existence or cadence is
//!   only known then. The scheduler is an injectable singleton, so any component can take
//!   `Arc<JobScheduler>` and schedule work.
//!
//! Both kinds share one [`JobRegistry`], so introspection ([`list_jobs`](JobScheduler::list_jobs),
//! [`job`](JobScheduler::job)) and control ([`pause`](JobScheduler::pause),
//! [`run_now`](JobScheduler::run_now), …) do not care where a job came from.
//!
//! The scheduler is a framework-internal component. Like the other framework singletons
//! (`ShutdownHandle`, `HookManager`, `ConfigReloader`), it hand-rolls its DI descriptors
//! rather than going through the `#[service]`/`#[hook]` macros — which root their generated
//! paths at the `upwell` facade and so cannot be used from a crate *below* it.
//!
//! Each job runs on its own [child token](CancellationToken::child_token) of the scheduler's
//! token, so cancelling one job stops only that job while dropping the scheduler (on shutdown)
//! cancels every one.

use std::any::{Any, TypeId};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;
use tracing::info;
use upwell_core::{DependencyDescriptor, ResolverCtx, ResolverCtxExt, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentSource, Result as DiResult, RootResolver, Singleton,
    dispatch_factory, factory_dependencies,
};
use upwell_hooks::{Error as HookError, HookDescriptor, HookKind, Result as HookResult, Startup};

use crate::descriptor::{JOBS, JobOutcome};
use crate::error::JobError;
use crate::log::{JobLogRecord, JobLogSink, NoopJobLogStore, SharedSink};
use crate::metrics::JobMetrics;
use crate::registry::{
    JobEntry, JobId, JobInfo, JobMetadata, JobRegistry, JobRunId, JobRunSummary, JobState,
};
use crate::run::{Runner, drive_job};
use crate::schedule::{JobOptions, Schedule, ScheduleError};

#[cfg(test)]
mod tests;

/// The stable component id of the [`JobScheduler`] singleton.
const SCHEDULER_ID: &str = "upwell:job-scheduler";

/// The display name of the [`JobScheduler`] singleton.
const SCHEDULER_NAME: &str = "JobScheduler";

/// A handle to one scheduled job, returned by [`JobScheduler::schedule`] and friends.
///
/// Use it to [`cancel`](Self::cancel) (un-register) the job or read its [`info`](Self::info).
/// Cloning shares the same underlying job; cancelling through any clone stops it. Dropping the
/// handle does **not** cancel the job — a scheduled job runs until explicitly cancelled or the
/// scheduler is dropped — so a fire-and-forget caller may discard it.
#[derive(Clone)]
pub struct JobHandle {
    entry: Arc<JobEntry>,
}

impl JobHandle {
    /// This job's identifier.
    pub fn id(&self) -> JobId {
        self.entry.id
    }

    /// Cancels (un-registers) the job: its loop stops at the next tick or wake, and it is
    /// removed from the scheduler. In-flight runs have their token cancelled. Idempotent.
    pub fn cancel(&self) {
        self.entry.begin_cancel();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.entry.token.is_cancelled()
    }

    /// A snapshot of the job's current state.
    pub fn info(&self) -> JobInfo {
        self.entry.info()
    }
}

/// Drives every job on its schedule.
///
/// Seeded as an injectable singleton by the [`JobsPlugin`](crate::plugin::JobsPlugin): its
/// `Startup` hook starts the static `#[job]`s, and any component can inject
/// `Arc<JobScheduler>` to [`schedule`](Self::schedule) more at run time, inspect them with
/// [`list_jobs`](Self::list_jobs), or control them ([`pause`](Self::pause),
/// [`run_now`](Self::run_now), …).
pub struct JobScheduler {
    root: RootResolver,
    /// Parent token: cancelling it (on `Drop`) stops every job. Each job holds a child.
    cancel: CancellationToken,
    /// Allocates [`JobId`]s.
    next_id: AtomicU64,
    /// Allocates [`JobRunId`]s, shared with every entry.
    run_ids: Arc<AtomicU64>,
    /// The registry of live jobs, shared with each job's loop for self-removal on exit.
    pub(crate) registry: Arc<JobRegistry>,
    /// The swappable log sink, shared with every job's loop.
    sink: SharedSink,
}

impl JobScheduler {
    /// The factory body: injects the framework-seeded [`RootResolver`].
    async fn create(root: RootResolver) -> Self {
        let sink: SharedSink = Arc::new(ArcSwap::from_pointee(
            Arc::new(NoopJobLogStore) as Arc<dyn JobLogSink>
        ));

        Self {
            root,
            cancel: CancellationToken::new(),
            next_id: AtomicU64::new(0),
            run_ids: Arc::new(AtomicU64::new(0)),
            registry: Arc::new(JobRegistry::default()),
            sink,
        }
    }

    // -- scheduling -------------------------------------------------------

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
    async fn start(&self) -> Result<(), ScheduleError> {
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

    // -- introspection ----------------------------------------------------

    /// A snapshot of every registered job.
    pub fn list_jobs(&self) -> Vec<JobInfo> {
        self.registry
            .entries()
            .iter()
            .map(|entry| entry.info())
            .collect()
    }

    /// A snapshot of the job with `id`, if registered.
    pub fn job(&self, id: JobId) -> Option<JobInfo> {
        self.registry.get(id).map(|entry| entry.info())
    }

    /// The recent run summaries for `id`, oldest first (empty if the job is unknown).
    pub fn recent_runs(&self, id: JobId) -> Vec<JobRunSummary> {
        self.registry
            .get(id)
            .map(|entry| entry.recent_runs())
            .unwrap_or_default()
    }

    /// An aggregate [`JobMetrics`] snapshot, for feeding an application's metrics/health system.
    pub fn metrics(&self) -> JobMetrics {
        let mut metrics = JobMetrics::default();

        for entry in self.registry.entries() {
            let info = entry.info();

            metrics.jobs_scheduled += 1;
            metrics.active_runs += entry.active();
            metrics.completed_runs += info.run_count;
            metrics.failed_runs += info.failure_count;
            metrics.skipped_ticks += info.skipped_count;

            if matches!(info.state, JobState::Paused) {
                metrics.paused_jobs += 1;
            }
        }

        metrics
    }

    /// The jobs overdue by more than `threshold` — a building block for a staleness policy.
    pub fn stale_jobs(&self, threshold: std::time::Duration) -> Vec<JobInfo> {
        self.list_jobs()
            .into_iter()
            .filter(|info| info.is_stale(threshold))
            .collect()
    }

    // -- manual runs ------------------------------------------------------

    /// Runs the job with `id` immediately, without waiting for its next occurrence. The run is
    /// recorded like a scheduled one but marked [`Manual`](crate::JobTrigger::Manual). It still
    /// respects the job's overlap policy, so under [`Skip`](crate::OverlapPolicy::Skip) the
    /// returned run may be skipped if a run is already active.
    pub async fn run_now(&self, id: JobId) -> Result<JobRunId, JobError> {
        let entry = self.registry.get(id).ok_or(JobError::UnknownJob(id))?;

        Ok(entry.trigger_manual())
    }

    /// Runs the first job named `name` immediately. See [`run_now`](Self::run_now).
    pub async fn run_named(&self, name: &str) -> Result<JobRunId, JobError> {
        let entry = self
            .registry
            .by_name(name)
            .ok_or_else(|| JobError::UnknownName(name.to_string()))?;

        Ok(entry.trigger_manual())
    }

    // -- control ----------------------------------------------------------

    /// Suspends scheduling for `id`: no new scheduled runs start until [`resume`](Self::resume),
    /// but a currently active run is not aborted.
    pub fn pause(&self, id: JobId) -> Result<(), JobError> {
        let entry = self.registry.get(id).ok_or(JobError::UnknownJob(id))?;
        entry.pause();

        Ok(())
    }

    /// Re-enables scheduling for `id` and recomputes its next fire time.
    pub fn resume(&self, id: JobId) -> Result<(), JobError> {
        let entry = self.registry.get(id).ok_or(JobError::UnknownJob(id))?;
        entry.resume();

        Ok(())
    }

    /// Changes the future cadence of `id` without losing its run history. The new schedule
    /// takes effect from the next fire.
    pub fn reschedule(&self, id: JobId, schedule: Schedule) -> Result<(), JobError> {
        let entry = self.registry.get(id).ok_or(JobError::UnknownJob(id))?;
        entry.reschedule(schedule);

        Ok(())
    }

    /// Cancels `id` and awaits its loop's teardown — useful for graceful shutdown and tests.
    pub async fn cancel_and_wait(&self, id: JobId) -> Result<(), JobError> {
        let entry = self.registry.get(id).ok_or(JobError::UnknownJob(id))?;
        entry.begin_cancel();
        entry.wait_done().await;

        Ok(())
    }

    // -- log capture ------------------------------------------------------

    /// Installs a [`JobLogSink`] for per-run log capture, replacing the default no-op sink.
    /// Pair it with a [`JobLogLayer`](crate::JobLogLayer) added to the tracing subscriber so
    /// the events emitted inside job runs are captured.
    pub fn set_log_sink(&self, sink: Arc<dyn JobLogSink>) {
        self.sink.store(Arc::new(sink));
    }

    /// The currently installed log sink.
    pub fn log_sink(&self) -> Arc<dyn JobLogSink> {
        let full = self.sink.load_full();

        (*full).clone()
    }

    /// The captured log records for `run_id`, up to `limit`, oldest first.
    pub async fn log_records(&self, run_id: JobRunId, limit: usize) -> Vec<JobLogRecord> {
        let sink = self.log_sink();

        sink.records(run_id, limit).await
    }
}

/// Cancels every loop when the scheduler is torn down (the root container drops on shutdown).
impl Drop for JobScheduler {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

// ---------------------------------------------------------------------------
// Hand-rolled DI descriptors (the `#[service]`/`#[hook]` macros cannot be used below the
// facade). This is the runtime metadata a `#[service] #[methods]` pair would otherwise emit.
// ---------------------------------------------------------------------------

impl Component for JobScheduler {
    type Handle = Arc<JobScheduler>;

    const ID: &'static str = SCHEDULER_ID;
    const NAME: &'static str = SCHEDULER_NAME;

    fn into_handle(self) -> Arc<JobScheduler> {
        Arc::new(self)
    }
}

/// Under `di-check`, the scheduler is plugin-seeded, so it is always provided — letting a user
/// component inject `Arc<JobScheduler>` to schedule jobs at run time.
#[cfg(feature = "di-check")]
impl upwell_di::Provide<JobScheduler> for upwell_di::Wiring {}

/// The scheduler's construction factory: resolves the injected [`RootResolver`] and builds it.
fn scheduler_construct<'a>(
    cx: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = DiResult<BoxedComponent>> + Send + 'a>> {
    dispatch_factory(JobScheduler::create, cx)
}

/// The scheduler's dependency edges, recovered from its factory's parameters.
fn scheduler_deps() -> Vec<DependencyDescriptor> {
    factory_dependencies(JobScheduler::create)
}

static SCHEDULER_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: scheduler_construct,
    dependencies: scheduler_deps,
    default: false,
}];

fn scheduler_factories() -> &'static [ComponentFactoryDescriptor] {
    &SCHEDULER_FACTORIES
}

/// The boxed future a hook call returns — the same shape `upwell-hooks` uses internally,
/// named here so the hand-rolled hook signature stays readable.
type HookCallFuture<'a> =
    Pin<Box<dyn Future<Output = HookResult<Box<dyn Any + Send>>> + Send + 'a>>;

/// The erased `Startup` hook: resolves the scheduler receiver through the component source and
/// starts its static jobs. Mirrors the body the `#[hook(Startup)]` macro would generate.
fn scheduler_startup_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> HookCallFuture<'a> {
    Box::pin(async move {
        let scheduler = ctx
            .get_resolver::<ComponentSource>()
            .and_then(|source| source.component::<JobScheduler>())
            .ok_or(HookError::MissingReceiver(SCHEDULER_NAME))?;

        scheduler
            .start()
            .await
            .map_err(|error| HookError::Other(Box::new(error)))?;

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn scheduler_startup_kind_ty() -> TypeId {
    TypeId::of::<Startup>()
}

fn scheduler_startup_deps() -> Vec<DependencyDescriptor> {
    Vec::new()
}

static SCHEDULER_HOOKS: [HookDescriptor; 1] = [HookDescriptor::new(
    0,
    TypeDescriptor::of::<JobScheduler>(SCHEDULER_NAME),
    <Startup as HookKind>::NAME,
    scheduler_startup_kind_ty,
    scheduler_startup_deps,
    scheduler_startup_call,
)];

fn scheduler_hooks() -> &'static [HookDescriptor] {
    &SCHEDULER_HOOKS
}

/// The [`ComponentDescriptor`] for the scheduler singleton, registered by the
/// [`JobsPlugin`](crate::plugin::JobsPlugin).
impl upwell_core::Descriptor<ComponentDescriptor> for JobScheduler {
    const DESCRIPTOR: ComponentDescriptor = ComponentDescriptor {
        id: SCHEDULER_ID,
        name: SCHEDULER_NAME,
        ty: TypeDescriptor::of::<JobScheduler>(SCHEDULER_NAME),
        scope: &Singleton,
        condition: None,
        factories: scheduler_factories,
        hooks: scheduler_hooks,
        generation_snapshot: ComponentDescriptor::typed_snapshot::<JobScheduler>(),
    };
}

#[cfg(test)]
pub(crate) fn scheduler_descriptor() -> ComponentDescriptor {
    <JobScheduler as upwell_core::Descriptor<ComponentDescriptor>>::DESCRIPTOR
}
