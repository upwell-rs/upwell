//! Detection of reloads requested from inside a running transition of the same runtime.
//!
//! A transition runs its hooks and candidate factories while holding the runtime's
//! non-reentrant serialization leases. [`ActiveTransition::scope`] marks every future
//! polled inside the transaction's task, so a nested reload of the same runtime fails
//! fast with [`ReentrantReload`](crate::Error::ReentrantReload) instead of awaiting a
//! lease its own caller holds.
//!
//! The marker is task-local. Work explicitly `tokio::spawn`ed from a hook or factory does
//! not inherit it: such a reload simply waits for the lease and runs after the
//! transaction completes, which cannot deadlock.

use std::future::Future;
use std::sync::Arc;

use super::TransitionAttemptId;

tokio::task_local! {
    static ACTIVE_TRANSITION: ActiveTransition;
}

/// The runtime transition the current task is running: the runtime's identity and the
/// attempt holding its leases.
#[derive(Clone)]
pub(crate) struct ActiveTransition {
    owner: Arc<()>,
    attempt: TransitionAttemptId,
}

impl ActiveTransition {
    pub(crate) fn new(owner: Arc<()>, attempt: TransitionAttemptId) -> Self {
        Self { owner, attempt }
    }

    /// Runs `future` with this transition marked as active for the current task. Every
    /// phase of a transaction that may run application code must run inside this scope.
    pub(crate) async fn scope<F: Future>(self, future: F) -> F::Output {
        ACTIVE_TRANSITION.scope(self, future).await
    }

    /// The attempt the current task is running for the runtime identified by `owner`,
    /// or `None` outside a transition or inside another runtime's transition.
    pub(crate) fn current_for(owner: &Arc<()>) -> Option<TransitionAttemptId> {
        ACTIVE_TRANSITION
            .try_with(|active| Arc::ptr_eq(&active.owner, owner).then_some(active.attempt))
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests;
