//! # Upwell Jobs
//!
//! A job scheduler for Upwell, integrated as a non-protocol
//! [`Plugin`](upwell_app::Plugin). Mark an `async` method on a `#[component]` with
//! `#[job(every = "..")]` or `#[job(cron = "..")]` inside a [`#[jobs]`](macro@jobs) impl block,
//! register [`JobsPlugin`], and the framework spawns a supervised background loop that runs the
//! method on schedule — resolving its `&self` receiver and any dependency parameters from the
//! DI container on each run.
//!
//! ```ignore
//! use upwell::jobs::{JobsPlugin, jobs};
//! use upwell::{component, Dep};
//!
//! #[component]
//! struct Reaper { db: Dep<Db> }
//!
//! #[jobs]
//! impl Reaper {
//!     // Reaches its dependencies through `&self`.
//!     #[job(every = "30s")]
//!     async fn sweep(&self) { self.db.snapshot().cleanup().await; }
//!
//!     // Extra parameters are resolved from the container per run — the shapes an
//!     // `#[init]` constructor takes (`Arc<T>`, `Dep<T>`, `Cfg<T>`, …), no wrapper needed.
//!     #[job(cron = "@hourly")]
//!     async fn report(&self, metrics: Dep<Metrics>) { metrics.snapshot().flush().await; }
//! }
//!
//! app.register_plugin::<JobsPlugin>();
//! ```
//!
//! ## Schedules
//!
//! - `every = ".."` — a fixed interval parsed by [`humantime`] (`"30s"`, `"5m"`, `"1h 30m"`,
//!   `"500ms"`), run on the monotonic clock. `months`/`years` are fixed approximations, so an
//!   interval is a constant wall-time gap, not a calendar cadence.
//! - `cron = ".."` — a cron expression or `@`-nickname (`"0 3 * * *"`, `"@hourly"`), run on the
//!   wall clock.
//!
//! Schedules parse at startup — an invalid one fails startup rather than silently never firing.
//! Each job runs on its own cancellation token, and a run is awaited sequentially, so a body
//! that outlasts its period simply skips the ticks it missed (the built-in overlap guard).
//!
//! ## Dynamic jobs
//!
//! Jobs whose existence or cadence is only known at run time (e.g. loaded from a database) are
//! added through [`JobScheduler::schedule`]. The scheduler is an injectable singleton, so any
//! component can take `Arc<JobScheduler>` and schedule work; the returned [`JobHandle`] cancels
//! (un-registers) it.
//!
//! ```ignore
//! let handle = scheduler.schedule_named("poll-upstream", Schedule::interval("5m")?, || async {
//!     poll_upstream().await?;
//!     Ok(())
//! });
//! // later …
//! handle.cancel();
//! ```
//!
//! [`schedule_named`](JobScheduler::schedule_named) names a runtime job so it stays
//! distinguishable in logs and introspection; [`schedule`](JobScheduler::schedule) is the
//! unnamed convenience wrapper.
//!
//! ## Introspection and control
//!
//! Every job — static or dynamic — lives in one registry, so the scheduler exposes a uniform
//! read/control surface: [`list_jobs`](JobScheduler::list_jobs) / [`job`](JobScheduler::job) /
//! [`recent_runs`](JobScheduler::recent_runs) return [`JobInfo`] / [`JobRunSummary`] snapshots
//! (state, next run, run/failure counts, recent outcomes); [`run_now`](JobScheduler::run_now)
//! and [`run_named`](JobScheduler::run_named) trigger a run immediately;
//! [`pause`](JobScheduler::pause) / [`resume`](JobScheduler::resume) /
//! [`reschedule`](JobScheduler::reschedule) / [`cancel_and_wait`](JobScheduler::cancel_and_wait)
//! provide softer operational control than a hard cancel; and [`metrics`](JobScheduler::metrics)
//! yields an aggregate [`JobMetrics`] snapshot to feed an application's own metrics/health
//! system (the crate hard-codes no backend and no health policy).
//!
//! ## Per-run context and progress
//!
//! A `#[job]` method may request the per-run [`JobRunContext`] as a parameter (threaded through
//! the run, not resolved from DI) to report structured [`JobProgress`] while it executes:
//!
//! ```ignore
//! #[job(every = "5m")]
//! async fn rebuild_index(&self, cx: JobRunContext) -> Result<()> {
//!     cx.progress(JobProgress::phase("loading")).await;
//!     Ok(())
//! }
//! ```
//!
//! ## Execution options
//!
//! Beyond *when* a job runs, `#[job(..)]` accepts *how* it runs: `run_on_startup`,
//! `timeout = ".."`, `jitter = ".."`, `max_runtime = ".."`, `overlap = <policy>`
//! ([`OverlapPolicy`]: `Skip`, `QueueOne`, `Allow`, `CancelPrevious`), and `tz = <zone>`
//! ([`JobTimezone`]). Every default preserves the original behaviour — no startup run, no
//! timeout, non-overlapping runs, cron computed from UTC — so adding options is opt-in.
//!
//! ```ignore
//! #[job(every = "30s", overlap = CancelPrevious, timeout = "20s")]
//! async fn sweep(&self) { /* … */ }
//! ```
//!
//! ## Per-run log capture
//!
//! Each run executes inside a `job.run` tracing span. A [`JobLogLayer`] added to the
//! application's subscriber captures the events emitted under that span into a pluggable
//! [`JobLogSink`] — the bundled [`InMemoryJobLogStore`] (bounded by runs/bytes/TTL), the
//! default [`NoopJobLogStore`], or an application-provided sink for out-of-process storage.
//! Install one with [`set_log_sink`](JobScheduler::set_log_sink) and read it back through
//! [`log_records`](JobScheduler::log_records).
//!
//! ## Standalone job runner
//!
//! The scheduler needs no network protocol. Because [`App::run`](upwell_app::App::run) runs
//! the startup hooks (which spawn the jobs) and then simply waits for shutdown — without serving
//! any endpoint — an app built with [`JobsPlugin`] and driven by `run()` (not `serve()`) is a
//! dedicated **scheduler process** with no request surface: a cron/worker daemon. Pair the
//! `jobs` feature with any protocol (its plugin only supplies the `App` type) and never call
//! `serve`; see `examples/jobs`.
//!
//! The scheduler is a framework singleton driven by a `Startup` hook (spawn) and its `Drop`
//! (graceful cancel of every loop); static jobs are discovered at link time, so registering the
//! plugin is all that is required.

pub mod descriptor;
pub mod error;
pub mod log;
#[cfg(feature = "tracing-subscriber")]
pub mod logging;
pub mod metrics;
pub mod plugin;
pub mod registry;
pub mod run;
pub mod schedule;
pub mod scheduler;
mod target;

pub use descriptor::{JOBS, JobCall, JobDescriptor, JobOutcome};
pub use error::JobError;
pub use log::{
    InMemoryJobLogStore, JobLogConfig, JobLogLayer, JobLogLevel, JobLogRecord, JobLogSink,
    NoopJobLogStore,
};
#[cfg(all(feature = "cli", feature = "tracing-subscriber"))]
pub use logging::configure_bootstrap_tracing;
#[cfg(feature = "tracing-subscriber")]
pub use logging::init_tracing;
pub use metrics::JobMetrics;
pub use plugin::JobsPlugin;
pub use registry::{
    JobId, JobInfo, JobMetadata, JobRunId, JobRunOutcome, JobRunSummary, JobState, JobTrigger,
};
pub use run::{JobProgress, JobRunContext};
pub use schedule::{
    JobOptions, JobTimezone, OverlapPolicy, Schedule, ScheduleError, ScheduleInfo, ScheduleKind,
};
pub use scheduler::{JobHandle, JobScheduler};
pub use target::JobTarget;

/// The `#[jobs]` impl-block macro and its `#[job]` method marker, owned by
/// `upwell-jobs-macros` and re-exported here (the plugin crate owns its macros).
pub use upwell_jobs_macros::{job, jobs};

/// Re-exported so `#[job]`-generated code can reach the `#[distributed_slice]` attribute
/// through a stable path (`upwell::jobs::linkme`).
#[doc(hidden)]
pub use linkme;
