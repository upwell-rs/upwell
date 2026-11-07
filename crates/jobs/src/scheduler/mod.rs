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

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use arc_swap::ArcSwap;
use tokio_util::sync::CancellationToken;
use upwell_di::RootResolver;

use crate::log::{JobLogSink, NoopJobLogStore, SharedSink};
use crate::registry::{JobEntry, JobId, JobInfo, JobRegistry};

mod control;
mod descriptor;
mod logging;
mod schedule;

#[cfg(test)]
pub(crate) use descriptor::scheduler_descriptor;

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
    /// Allocates [`JobRunId`](crate::registry::JobRunId)s, shared with every entry.
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
}

/// Cancels every loop when the scheduler is torn down (the root container drops on shutdown).
impl Drop for JobScheduler {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
