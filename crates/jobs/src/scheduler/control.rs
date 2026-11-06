//! Introspecting, triggering and controlling registered jobs.

use super::JobScheduler;
use crate::error::JobError;
use crate::metrics::JobMetrics;
use crate::registry::{JobId, JobInfo, JobRunId, JobRunSummary, JobState};
use crate::schedule::Schedule;
impl JobScheduler {
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
}
