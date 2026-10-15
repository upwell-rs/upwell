//! Detection of reloads requested from inside a running transition of the same runtime.
//!
//! A transition runs its hooks and candidate factories while holding the runtime's
//! non-reentrant serialization leases. [`ActiveTransition::scope`] marks every future
//! polled inside the transaction's task, so a nested reload of the same runtime fails
//! fast with [`ReentrantReload`](crate::Error::ReentrantReload) instead of awaiting a
//! lease its own caller holds.
//!
//! Markers nest: a transaction of one runtime that reloads another runtime inline (allowed
//! by this check, though the inner reload still waits if that runtime's leases are held
//! by another task) keeps its own marker active underneath the inner one, so an inner
//! transaction that reloads the outer runtime (an `A -> B -> A` cycle on one task) is
//! still rejected.
//!
//! The marker only sees the current task. Two transactions on different tasks that each
//! reload the other's runtime inline (`A -> B` on one task while `B -> A` runs on another)
//! wait on each other's leases and deadlock; that cross-task lock-order cycle is not
//! detected. Avoid inline cross-runtime reloads from hooks and factories, or spawn them
//! without awaiting them.
//!
//! The marker is task-local. Work explicitly `tokio::spawn`ed from a hook or factory does
//! not inherit it: such a reload waits for the lease and runs after the transaction
//! completes, which cannot deadlock as long as the transaction does not itself await the
//! spawned work (see the reentrancy section of the [`reload`](super::reload) module).

use std::future::Future;
use std::sync::Arc;

use super::TransitionAttemptId;

tokio::task_local! {
    static ACTIVE_TRANSITIONS: Arc<[ActiveTransition]>;
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

    /// Runs `future` with this transition marked as active for the current task, on top
    /// of any transitions the task is already running. Every phase of a transaction that
    /// may run application code must run inside this scope.
    pub(crate) async fn scope<F: Future>(self, future: F) -> F::Output {
        let mut active = ACTIVE_TRANSITIONS
            .try_with(|outer| outer.to_vec())
            .unwrap_or_default();

        active.push(self);

        ACTIVE_TRANSITIONS.scope(active.into(), future).await
    }

    /// The attempt the current task is running for the runtime identified by `owner` at
    /// any nesting depth, or `None` when the task runs no transition of that runtime.
    pub(crate) fn current_for(owner: &Arc<()>) -> Option<TransitionAttemptId> {
        ACTIVE_TRANSITIONS
            .try_with(|active| {
                active
                    .iter()
                    .find(|transition| Arc::ptr_eq(&transition.owner, owner))
                    .map(|transition| transition.attempt)
            })
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests;
