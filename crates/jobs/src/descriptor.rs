//! The job metadata a `#[job]` method registers, and the link-time slice they collect into.

use std::future::Future;
use std::pin::Pin;

use upwell_core::{ItemSource, TypeDescriptor};
use upwell_di::RootResolver;

use crate::run::JobRunContext;
use crate::schedule::{JobOptions, ScheduleKind};

/// The outcome of one job run: `Ok(())`, or any error the handler produced (a resolution
/// failure, or a domain error from a `Result`-returning `#[job]`). Boxed because the job
/// layer sits below any particular domain and cannot name a user's error type.
pub type JobOutcome = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// The erased invocation of one `#[job]` method.
///
/// The macro generates this per job: given the [`RootResolver`] and the per-run
/// [`JobRunContext`], it resolves the method's `&self` receiver and each dependency parameter
/// from the root scope (passing the context straight through for a `JobRunContext` parameter),
/// runs the method, and normalizes the result into a [`JobOutcome`]. It owns a cloned resolver,
/// so the returned future is `'static` and can drive a spawned task.
pub type JobCall =
    fn(RootResolver, JobRunContext) -> Pin<Box<dyn Future<Output = JobOutcome> + Send + 'static>>;

/// Static metadata for one `#[job]` method, registered into the [`JOBS`] slice.
pub struct JobDescriptor {
    /// A stable name for logging and diagnostics — `"Type::method"`.
    pub name: &'static str,
    /// The component the job method is defined on (for introspection).
    pub component_ty: TypeDescriptor,
    /// The raw schedule literal from the attribute (`"30s"`, `"0 3 * * *"`, `"@hourly"`).
    pub schedule: &'static str,
    /// Whether `schedule` is an interval or a cron expression.
    pub kind: ScheduleKind,
    /// The erased call that runs the job.
    pub call: JobCall,
    /// The job's execution options (overlap policy, timeout, jitter, …), parsed from the
    /// extra `#[job(..)]` keys. [`JobOptions::default`] preserves the original behaviour.
    pub options: JobOptions,
    /// The job method's declaration.
    pub source: ItemSource,
}

impl std::fmt::Debug for JobDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobDescriptor")
            .field("name", &self.name)
            .field("schedule", &self.schedule)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Link-time registry of every discovered [`JobDescriptor`].
///
/// A `#[job]` method appends its descriptor here via `#[linkme::distributed_slice(JOBS)]`;
/// the [`JobScheduler`](crate::scheduler::JobScheduler) reads the assembled slice at startup.
#[linkme::distributed_slice]
pub static JOBS: [JobDescriptor];
