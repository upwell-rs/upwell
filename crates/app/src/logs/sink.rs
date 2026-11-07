//! The place one scope instance's captured events go.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{Semaphore, mpsc};
use tracing::Event;
use tracing::level_filters::LevelFilter;
use upwell_core::ScopeId;

use super::stream::LagCounter;
use super::{LogField, LogOptions, LogRecord, ScopeLogStream};

/// The sink of one scope instance, shared by its span and its subscribers.
///
/// Records start flowing on the first subscriber: until then an event costs a level check and
/// one atomic read. Each subscriber has its own bounded buffer, and a full buffer drops the
/// record and counts it, so the logging thread never waits.
pub struct ScopeLogSink {
    scope: ScopeId,
    params: OnceLock<Arc<[LogField]>>,
    level: Option<LevelFilter>,
    buffer: usize,
    watchers: AtomicUsize,
    attached: AtomicBool,
    subscribers: Mutex<Option<Vec<Subscriber>>>,
}

/// One subscriber's end of the sink.
struct Subscriber {
    sender: mpsc::Sender<LogRecord>,
    lag: Arc<LagCounter>,
}

impl ScopeLogSink {
    /// A sink for an instance of `scope` with the loggable `params`, capturing as `options`
    /// say.
    pub(super) fn new(scope: ScopeId, params: Arc<[LogField]>, options: &LogOptions) -> Self {
        Self {
            scope,
            params: OnceLock::from(params),
            level: options.level,
            buffer: options.buffer.get().min(Semaphore::MAX_PERMITS),
            watchers: AtomicUsize::new(0),
            attached: AtomicBool::new(false),
            subscribers: Mutex::new(Some(Vec::new())),
        }
    }

    /// A sink whose loggable params are not known yet: [`describe`](Self::describe) supplies
    /// them before the instance emits its first event.
    pub(super) fn deferred(scope: ScopeId, options: &LogOptions) -> Self {
        let mut sink = Self::new(scope, Arc::from([]), options);

        sink.params = OnceLock::new();

        sink
    }

    /// Sets the loggable params of the instance unless they are set already.
    pub(super) fn describe(&self, params: impl FnOnce() -> Arc<[LogField]>) {
        self.params.get_or_init(params);
    }

    /// A stream of the records the instance captures from now on, ending when the instance
    /// closes. Taken after the instance closed, it ends at once.
    pub fn subscribe(&self) -> ScopeLogStream {
        let (sender, receiver) = mpsc::channel(self.buffer);
        let lag = Arc::new(LagCounter::default());
        let mut subscribers = self.subscribers.lock().expect("sink lock not poisoned");

        if let Some(open) = subscribers.as_mut() {
            open.retain(|subscriber| !subscriber.sender.is_closed());
            open.push(Subscriber {
                sender,
                lag: Arc::clone(&lag),
            });
            self.watchers.store(open.len(), Ordering::Release);
        }

        ScopeLogStream::new(receiver, lag)
    }

    /// Whether a layer took the sink when the instance's span opened. False when no
    /// [`ScopeLogLayer`](super::ScopeLogLayer) is installed, or its filter rejects the span:
    /// the instance then captures nothing.
    pub fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Acquire)
    }

    /// Ends every stream once it has drained, and refuses new subscribers' records.
    pub(crate) fn close(&self) {
        let closed = self
            .subscribers
            .lock()
            .expect("sink lock not poisoned")
            .take();

        drop(closed);
        self.watchers.store(0, Ordering::Release);
    }

    #[cfg(test)]
    /// How many subscribers the sink still holds, closed ones included until it next prunes.
    pub(super) fn watching(&self) -> usize {
        self.watchers.load(Ordering::Acquire)
    }

    pub(super) fn attach(&self) {
        self.attached.store(true, Ordering::Release);
    }

    /// Delivers `event` to every subscriber, unless nobody watches or the sink's capture level
    /// rejects it.
    pub(super) fn deliver(&self, event: &Event<'_>) {
        if self.watchers.load(Ordering::Acquire) == 0 {
            return;
        }

        if self
            .level
            .is_some_and(|level| *event.metadata().level() > level)
        {
            return;
        }

        let params = self.params.get().map_or_else(|| Arc::from([]), Arc::clone);
        let record = LogRecord::from_event(event, self.scope, params);
        let targets = self.targets();

        if targets.is_empty() {
            return;
        }

        let gone = targets
            .iter()
            .filter(|target| !target.offer(&record))
            .count();

        drop(targets);

        if gone > 0 {
            self.prune();
        }
    }

    /// The subscribers' ends, cloned so records are sent and readers woken without the lock:
    /// a woken reader may call back into the sink.
    fn targets(&self) -> Vec<Subscriber> {
        let guard = self.subscribers.lock().expect("sink lock not poisoned");

        guard
            .as_ref()
            .map(|subscribers| subscribers.iter().map(Subscriber::share).collect())
            .unwrap_or_default()
    }

    /// Drops the subscribers that went away.
    fn prune(&self) {
        let mut guard = self.subscribers.lock().expect("sink lock not poisoned");

        if let Some(subscribers) = guard.as_mut() {
            subscribers.retain(|subscriber| !subscriber.sender.is_closed());
            self.watchers.store(subscribers.len(), Ordering::Release);
        }
    }
}

impl Subscriber {
    fn share(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            lag: Arc::clone(&self.lag),
        }
    }

    /// Hands `record` to the subscriber without waiting. False once it went away.
    fn offer(&self, record: &LogRecord) -> bool {
        match self.sender.try_send(record.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.lag.record_drop();

                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}
