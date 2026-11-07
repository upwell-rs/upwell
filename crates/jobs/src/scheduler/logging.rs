//! Per-run log capture.

use std::sync::Arc;

use super::JobScheduler;
use crate::log::{JobLogRecord, JobLogSink};
use crate::registry::JobRunId;
impl JobScheduler {
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
