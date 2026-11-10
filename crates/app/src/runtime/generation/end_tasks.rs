//! The tasks that deliver scope-end hooks, tracked so a graceful shutdown can wait for them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

/// The end-hook tasks of one runtime that have not finished.
///
/// A task counts from the moment its instance ends until its [`EndTask`] drops, whether it
/// finished, was cancelled or never ran, so a drain never waits on a task that is gone.
#[derive(Default)]
pub(crate) struct EndTasks {
    pending: AtomicUsize,
    idle: Notify,
}

/// One pending end-hook task, counted until it drops.
pub(crate) struct EndTask {
    tasks: Arc<EndTasks>,
}

impl EndTasks {
    /// Counts a task that is about to be spawned.
    pub(crate) fn track(self: &Arc<Self>) -> EndTask {
        self.pending.fetch_add(1, Ordering::AcqRel);

        EndTask {
            tasks: Arc::clone(self),
        }
    }

    /// Waits until no task is pending, or `limit` passes. True when none is pending.
    pub(crate) async fn drain(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.idle()).await.is_ok()
    }

    async fn idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }

            notified.await;
        }
    }
}

impl Drop for EndTask {
    fn drop(&mut self) {
        if self.tasks.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.tasks.idle.notify_waiters();
        }
    }
}
