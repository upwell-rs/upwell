//! Publishing a prepared runtime generation: the commit and its single publication step.

use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::OwnedMutexGuard;
use upwell_core::RuntimeGenerationId;

use super::{RuntimeGeneration, RuntimeTransitionCoordinator, RuntimeView, TransitionAttemptId};
use crate::transition::CommitActions;

#[derive(Clone)]
pub(super) struct RuntimePublication {
    pub(super) current: Arc<ArcSwap<RuntimeGeneration>>,
}

impl RuntimePublication {
    pub(super) fn new(initial: RuntimeGeneration) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(initial)),
        }
    }

    pub(super) fn pin(&self) -> RuntimeView {
        RuntimeView::from_generation(self.current.load_full())
    }

    pub(super) fn publish(
        &self,
        attempt: TransitionAttemptId,
        base: &Arc<RuntimeGeneration>,
        candidate: Arc<RuntimeGeneration>,
    ) -> Result<RuntimeView, StaleRuntimeProposal> {
        let previous = self.current.compare_and_swap(base, Arc::clone(&candidate));

        if !Arc::ptr_eq(&previous, base) {
            return Err(StaleRuntimeProposal {
                attempt,
                base: base.id,
                current: previous.id,
            });
        }

        Ok(RuntimeView::from_generation(candidate))
    }
}

/// A validated, stamped candidate generation awaiting terminal publication.
///
/// Prepared by [`RuntimeTransition::prepare_commit`]; owns the sole-writer lease, the
/// attempt identity, the exact base generation, and the fully committed candidate.
pub(crate) struct PreparedRuntimeCommit {
    pub(super) coordinator: RuntimeTransitionCoordinator,
    pub(super) writer: OwnedMutexGuard<()>,
    pub(super) attempt: TransitionAttemptId,
    pub(super) base: Arc<RuntimeGeneration>,
    pub(super) candidate: Arc<RuntimeGeneration>,
    pub(super) commit_actions: CommitActions,
}

impl PreparedRuntimeCommit {
    /// Publishes the validated candidate, applies its prepared live rebinds and hot
    /// updates, runs the synchronous legacy-config compatibility callback while still
    /// holding the writer, then releases the writer.
    ///
    /// Ordering: the already validated candidate generation becomes current first, and the
    /// base is recorded as displaced while this commit still pins it. Then
    /// every prepared live rebind swaps its cell and every prepared hot update applies,
    /// then the callback observes the new generation through the coordinator, and only
    /// then does the writer lease end. Publication cannot fail after `prepare_commit` accepted the
    /// candidate: the writer is the sole publication serializer, so a compare-and-swap
    /// failure would be an internal invariant violation, not a recoverable error.
    pub(crate) fn commit_with(self, commit_compat: impl FnOnce()) -> RuntimeView {
        let PreparedRuntimeCommit {
            coordinator,
            writer,
            attempt,
            base,
            candidate,
            commit_actions,
        } = self;

        let view = coordinator
            .publication
            .publish(attempt, &base, candidate)
            .expect("validated candidate must publish under the sole writer");

        coordinator.displaced.displace(base.id, attempt);

        commit_actions.commit();
        commit_compat();

        drop(writer);

        view
    }
}

/// A prepared transition no longer has the active generation as its exact base.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "transition attempt {attempt:?} was based on generation {base:?}, but generation {current:?} is current"
)]
pub(crate) struct StaleRuntimeProposal {
    pub attempt: TransitionAttemptId,
    pub base: RuntimeGenerationId,
    pub current: RuntimeGenerationId,
}
