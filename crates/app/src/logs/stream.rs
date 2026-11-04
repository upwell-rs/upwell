//! The stream of one subscriber's records.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::mpsc;

use super::{LogRecord, ScopeLog};

/// The records one scope instance captures, as owned [`ScopeLog`] items.
///
/// It ends when the instance closes, so collecting it yields the whole log. When the
/// subscriber falls behind its bounded buffer, records are dropped instead of slowing the
/// instance down, and a [`ScopeLog::Lagged`] reports how many once the buffered records have
/// been read.
pub struct ScopeLogStream {
    receiver: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicUsize>,
}

impl ScopeLogStream {
    pub(super) fn new(receiver: mpsc::Receiver<LogRecord>, dropped: Arc<AtomicUsize>) -> Self {
        Self { receiver, dropped }
    }

    /// A stream that has already ended, for an instance that captures nothing.
    pub fn ended() -> Self {
        let (_, receiver) = mpsc::channel(1);

        Self::new(receiver, Arc::new(AtomicUsize::new(0)))
    }

    fn lagged(&self) -> Option<ScopeLog> {
        match self.dropped.swap(0, Ordering::AcqRel) {
            0 => None,
            dropped => Some(ScopeLog::Lagged(dropped)),
        }
    }
}

impl Stream for ScopeLogStream {
    type Item = ScopeLog;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(record)) => Poll::Ready(Some(ScopeLog::Record(record))),
            Poll::Ready(None) => Poll::Ready(self.lagged()),
            Poll::Pending => match self.lagged() {
                Some(lagged) => Poll::Ready(Some(lagged)),
                None => Poll::Pending,
            },
        }
    }
}
