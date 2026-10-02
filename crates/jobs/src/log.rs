//! Generic, pluggable per-run log capture.
//!
//! Log storage is deliberately behind a trait ([`JobLogSink`]) so `upwell-jobs` never
//! couples to a particular backend: the built-in [`InMemoryJobLogStore`] is a bounded local
//! buffer for development, [`NoopJobLogStore`] is the do-nothing default, and an application
//! can supply its own sink (a database, object store, or remote pipeline) through
//! [`JobScheduler::set_log_sink`](crate::JobScheduler::set_log_sink).
//!
//! Capture itself is a normal [`tracing_subscriber::Layer`] ([`JobLogLayer`]): it records the
//! events emitted inside a job's [`job.run`](crate::JobRunContext) span, so it composes with
//! the application's existing tracing setup rather than requiring a bespoke per-run subscriber.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::sync::mpsc::{Sender, channel};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::registry::{JobId, JobRunId};

#[cfg(test)]
mod tests;

/// The name of the per-run span the capture layer correlates events against.
pub(crate) const RUN_SPAN_NAME: &str = "job.run";

/// The swappable sink slot the scheduler holds. Swapping it (via
/// [`JobScheduler::set_log_sink`](crate::JobScheduler::set_log_sink)) is observed by every
/// subsequent query without re-seeding. The inner `Arc<dyn JobLogSink>` is `Sized` (a fat
/// pointer), so double-wrapping it clears `ArcSwap`'s `Sized` bound — the same
/// `Arc<ArcSwap<Arc<T>>>` shape used elsewhere for live, reloadable values.
pub(crate) type SharedSink = Arc<ArcSwap<Arc<dyn JobLogSink>>>;

/// A single captured log event belonging to one job run.
#[derive(Debug, Clone)]
pub struct JobLogRecord {
    /// The run the event was emitted during.
    pub run_id: JobRunId,
    /// The job the run belongs to.
    pub job_id: JobId,
    /// The job's stable name.
    pub job_name: Arc<str>,
    /// When the event was emitted.
    pub timestamp: SystemTime,
    /// The event's severity.
    pub level: JobLogLevel,
    /// The event's tracing target.
    pub target: String,
    /// The rendered event message (its `message` field, plus any other fields appended).
    pub message: String,
}

impl JobLogRecord {
    /// The record's on-the-wire size, used to enforce
    /// [`JobLogConfig::max_bytes_per_run`]: the message plus target bytes.
    fn size(&self) -> usize {
        self.message.len() + self.target.len()
    }
}

/// A `Copy` mirror of [`tracing::Level`], so a [`JobLogRecord`] carries a severity without a
/// consumer depending on `tracing` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<tracing::Level> for JobLogLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::TRACE => JobLogLevel::Trace,
            tracing::Level::DEBUG => JobLogLevel::Debug,
            tracing::Level::INFO => JobLogLevel::Info,
            tracing::Level::WARN => JobLogLevel::Warn,
            tracing::Level::ERROR => JobLogLevel::Error,
        }
    }
}

/// Where captured [`JobLogRecord`]s are stored. The layer records into it; introspection
/// reads back through it.
///
/// Implementations must be cheap to clone-share (`Arc`-held) and safe to call concurrently.
// `async_trait` stamps `#[must_use]` on the generated methods whose boxed-future
// return type is already `must_use`; newer clippy flags that as `double_must_use`.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait JobLogSink: Send + Sync + 'static {
    /// Stores one captured event.
    async fn record(&self, event: JobLogRecord);

    /// Returns up to `limit` of the most recent records for `run_id`, oldest first.
    async fn records(&self, run_id: JobRunId, limit: usize) -> Vec<JobLogRecord>;
}

/// Configuration for the bounded [`InMemoryJobLogStore`].
#[derive(Debug, Clone)]
pub struct JobLogConfig {
    /// Whether capture is enabled at all. When `false`, the store drops every record.
    pub enabled: bool,
    /// The maximum number of distinct runs retained; the oldest run is evicted past this.
    pub max_runs: usize,
    /// The maximum total record bytes retained per run; the oldest record in a run is
    /// dropped once this is exceeded.
    pub max_bytes_per_run: usize,
    /// How long a run's records are retained before they are considered expired.
    pub ttl: Duration,
    /// The minimum severity the [`JobLogLayer`] captures.
    pub capture_level: tracing::Level,
}

impl Default for JobLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_runs: 128,
            max_bytes_per_run: 64 * 1024,
            ttl: Duration::from_secs(3600),
            capture_level: tracing::Level::INFO,
        }
    }
}

/// One run's buffered records, plus the bookkeeping the bounds are enforced against.
struct RunLog {
    records: VecDeque<JobLogRecord>,
    bytes: usize,
    first_seen: SystemTime,
}

/// A bounded in-memory [`JobLogSink`] for local observability.
///
/// Bounds are enforced on every [`record`](JobLogSink::record): expired runs (older than
/// [`ttl`](JobLogConfig::ttl)) are dropped, a run over [`max_bytes_per_run`](JobLogConfig::max_bytes_per_run)
/// sheds its oldest records, and the oldest run is evicted once [`max_runs`](JobLogConfig::max_runs)
/// is exceeded. The most recent record of a run is always retained, so a single record larger
/// than `max_bytes_per_run` is kept in full rather than leaving the run with no logs at all.
/// Not intended for durable or out-of-process storage — supply your own [`JobLogSink`] for that.
pub struct InMemoryJobLogStore {
    config: JobLogConfig,
    runs: Mutex<HashMap<JobRunId, RunLog>>,
    /// Insertion order of live runs, for O(1) oldest-run eviction.
    order: Mutex<VecDeque<JobRunId>>,
}

impl InMemoryJobLogStore {
    /// A store bounded by `config`.
    pub fn new(config: JobLogConfig) -> Self {
        Self {
            config,
            runs: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
        }
    }

    /// A store with [`JobLogConfig::default`] bounds.
    pub fn with_defaults() -> Self {
        Self::new(JobLogConfig::default())
    }
}

#[async_trait]
impl JobLogSink for InMemoryJobLogStore {
    async fn record(&self, event: JobLogRecord) {
        if !self.config.enabled {
            return;
        }

        let now = event.timestamp;
        let run_id = event.run_id;
        let size = event.size();

        let mut runs = self.runs.lock().expect("job log store not poisoned");
        let mut order = self.order.lock().expect("job log order not poisoned");

        // Drop expired runs before inserting, so the bounds reflect only live data.
        runs.retain(|_, log| {
            now.duration_since(log.first_seen)
                .map(|age| age <= self.config.ttl)
                .unwrap_or(true)
        });
        order.retain(|id| runs.contains_key(id));

        let log = runs.entry(run_id).or_insert_with(|| {
            order.push_back(run_id);

            RunLog {
                records: VecDeque::new(),
                bytes: 0,
                first_seen: now,
            }
        });

        log.records.push_back(event);
        log.bytes += size;

        while log.bytes > self.config.max_bytes_per_run && log.records.len() > 1 {
            if let Some(dropped) = log.records.pop_front() {
                log.bytes = log.bytes.saturating_sub(dropped.size());
            }
        }

        while order.len() > self.config.max_runs {
            if let Some(evicted) = order.pop_front() {
                runs.remove(&evicted);
            }
        }
    }

    async fn records(&self, run_id: JobRunId, limit: usize) -> Vec<JobLogRecord> {
        let runs = self.runs.lock().expect("job log store not poisoned");

        let Some(log) = runs.get(&run_id) else {
            return Vec::new();
        };

        let skip = log.records.len().saturating_sub(limit);

        log.records.iter().skip(skip).cloned().collect()
    }
}

/// A [`JobLogSink`] that discards everything — the default when capture is disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopJobLogStore;

#[async_trait]
impl JobLogSink for NoopJobLogStore {
    async fn record(&self, _event: JobLogRecord) {}

    async fn records(&self, _run_id: JobRunId, _limit: usize) -> Vec<JobLogRecord> {
        Vec::new()
    }
}

/// The per-run metadata parsed off a [`job.run`](RUN_SPAN_NAME) span and stashed in its
/// extensions, so an event nested under it can be attributed without re-parsing.
#[derive(Clone)]
struct RunSpanFields {
    run_id: JobRunId,
    job_id: JobId,
    job_name: Arc<str>,
}

/// Visitor that pulls the `run_id`/`job_id`/`job_name` fields off a `job.run` span's
/// attributes.
#[derive(Default)]
struct RunFieldsVisitor {
    run_id: Option<u64>,
    job_id: Option<u64>,
    job_name: Option<String>,
}

impl Visit for RunFieldsVisitor {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "run_id" => self.run_id = Some(value),
            "job_id" => self.job_id = Some(value),

            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "job_name" {
            self.job_name = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "job_name" {
            self.job_name = Some(format!("{value:?}"));
        }
    }
}

/// Visitor that renders an event's fields into a single message string: the `message` field
/// verbatim, with any remaining fields appended as `key=value`.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn render(mut self) -> String {
        if !self.fields.is_empty() {
            if !self.message.is_empty() {
                self.message.push(' ');
            }

            self.message.push_str(self.fields.trim_start());
        }

        self.message
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

/// The capture forwarding channel's capacity. Bounds in-flight records so a burst of job logs
/// (or a slow sink) cannot grow the queue without limit; excess records are dropped rather than
/// buffered, keeping capture best-effort and memory-safe.
const CAPTURE_CHANNEL_CAPACITY: usize = 8192;

/// A [`tracing_subscriber::Layer`] that captures the events emitted inside a
/// [`job.run`](RUN_SPAN_NAME) span into a [`JobLogSink`].
///
/// Records are forwarded to the (async) sink through a bounded channel drained by a background
/// task, so the synchronous tracing path never blocks on storage and the sink may be an
/// out-of-process backend. If the channel fills (a very chatty job, or a slow sink), further
/// records are dropped rather than buffered — capture is best-effort. Construct within a Tokio
/// runtime — the drain task is spawned on `new`.
pub struct JobLogLayer {
    tx: Sender<JobLogRecord>,
    capture_level: tracing::Level,
}

impl JobLogLayer {
    /// Builds a layer forwarding captured records to `sink`, capturing events at or above
    /// `capture_level`.
    pub fn new(sink: Arc<dyn JobLogSink>, capture_level: tracing::Level) -> Self {
        let (tx, mut rx) = channel::<JobLogRecord>(CAPTURE_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                sink.record(record).await;
            }
        });

        Self { tx, capture_level }
    }
}

impl<S> Layer<S> for JobLogLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        if attrs.metadata().name() != RUN_SPAN_NAME {
            return;
        }

        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = RunFieldsVisitor::default();
        attrs.record(&mut visitor);

        if let (Some(run_id), Some(job_id), Some(job_name)) =
            (visitor.run_id, visitor.job_id, visitor.job_name)
        {
            span.extensions_mut().insert(RunSpanFields {
                run_id: JobRunId::from_raw(run_id),
                job_id: JobId::from_raw(job_id),
                job_name: Arc::from(job_name.as_str()),
            });
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        if *event.metadata().level() > self.capture_level {
            return;
        }

        // Find the nearest enclosing `job.run` span's stashed fields.
        let Some(run) = ctx
            .event_scope(event)
            .into_iter()
            .flatten()
            .find_map(|span| span.extensions().get::<RunSpanFields>().cloned())
        else {
            return;
        };

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let record = JobLogRecord {
            run_id: run.run_id,
            job_id: run.job_id,
            job_name: run.job_name,
            timestamp: SystemTime::now(),
            level: (*event.metadata().level()).into(),
            target: event.metadata().target().to_string(),
            message: visitor.render(),
        };

        // A full or closed channel simply drops the record — capture is best-effort, and
        // `try_send` never blocks the synchronous tracing path.
        let _ = self.tx.try_send(record);
    }
}
