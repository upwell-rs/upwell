//! The transition coordinator that serializes graph-changing triggers, and one in-flight transition.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, OwnedMutexGuard};
use upwell_core::RuntimeGenerationId;

use super::release::{DisplacedGeneration, DisplacedGenerations};
use super::{
    PreparedRuntimeCommit, PreparedRuntimeGeneration, RuntimeGeneration, RuntimePublication,
    RuntimeView, StaleRuntimeProposal, TransitionAttemptId,
};
use crate::runtime::reentrancy::ActiveTransition;

/// Sole serializer and atomic publication owner for runtime-generation transitions.
#[derive(Clone)]
pub(crate) struct RuntimeTransitionCoordinator {
    owner: Arc<()>,
    pub(super) publication: RuntimePublication,
    pub(super) displaced: Arc<DisplacedGenerations>,
    pub(super) writer: Arc<Mutex<()>>,
    next_attempt: Arc<AtomicU64>,
}

/// A non-owning [`RuntimeTransitionCoordinator`] reference that never keeps a committed
/// generation alive.
#[derive(Clone)]
pub(crate) struct WeakTransitionCoordinator {
    owner: Arc<()>,
    current: Weak<ArcSwap<RuntimeGeneration>>,
    displaced: Arc<DisplacedGenerations>,
    writer: Arc<Mutex<()>>,
    next_attempt: Arc<AtomicU64>,
}

impl WeakTransitionCoordinator {
    /// The coordinator, while its publication cell is still owned by a runtime.
    pub(crate) fn upgrade(&self) -> Option<RuntimeTransitionCoordinator> {
        let current = self.current.upgrade()?;

        Some(RuntimeTransitionCoordinator {
            owner: Arc::clone(&self.owner),
            publication: RuntimePublication { current },
            displaced: Arc::clone(&self.displaced),
            writer: Arc::clone(&self.writer),
            next_attempt: Arc::clone(&self.next_attempt),
        })
    }
}

impl RuntimeTransitionCoordinator {
    pub(crate) fn new(initial: PreparedRuntimeGeneration) -> Self {
        let owner = Arc::new(());
        let displaced = Arc::default();
        let (initial, _) =
            initial.commit(Arc::clone(&owner), RuntimeGenerationId::INITIAL, &displaced);
        let publication = RuntimePublication::new(initial);

        Self {
            owner,
            publication,
            displaced,
            writer: Arc::new(Mutex::new(())),
            next_attempt: Arc::new(AtomicU64::new(1)),
        }
    }

    /// A non-owning reference for handles seeded into the coordinator's own generations.
    pub(crate) fn downgrade(&self) -> WeakTransitionCoordinator {
        WeakTransitionCoordinator {
            owner: Arc::clone(&self.owner),
            current: Arc::downgrade(&self.publication.current),
            displaced: Arc::clone(&self.displaced),
            writer: Arc::clone(&self.writer),
            next_attempt: Arc::clone(&self.next_attempt),
        }
    }

    /// Pins the complete currently committed runtime generation.
    pub(crate) fn current(&self) -> RuntimeView {
        self.publication.pin()
    }

    /// The displaced generations whose roots are still alive, oldest first.
    pub(crate) fn displaced_generations(&self) -> Vec<DisplacedGeneration> {
        self.displaced.alive_generations()
    }

    /// The runtime identity shared by every handle of this coordinator.
    pub(crate) fn owner(&self) -> &Arc<()> {
        &self.owner
    }

    pub(crate) fn owns(&self, generation: &Arc<RuntimeGeneration>) -> bool {
        Arc::ptr_eq(&self.owner, &generation.owner)
    }

    /// Starts one serialized transition attempt against the exact current generation.
    pub(crate) async fn begin(&self) -> RuntimeTransition {
        let writer = Arc::clone(&self.writer).lock_owned().await;
        let attempt = TransitionAttemptId::new(self.next_attempt.fetch_add(1, Ordering::Relaxed));
        let base = self.publication.pin();

        RuntimeTransition {
            coordinator: self.clone(),
            writer,
            attempt,
            base,
        }
    }
}

/// One serialized transition attempt bound to an exact base generation.
pub(crate) struct RuntimeTransition {
    coordinator: RuntimeTransitionCoordinator,
    writer: OwnedMutexGuard<()>,
    attempt: TransitionAttemptId,
    base: RuntimeView,
}

impl RuntimeTransition {
    /// The observability identity allocated for this attempt.
    pub(crate) const fn attempt(&self) -> TransitionAttemptId {
        self.attempt
    }

    /// The marker identifying this attempt as the active transition of its runtime.
    pub(crate) fn active(&self) -> ActiveTransition {
        ActiveTransition::new(Arc::clone(&self.coordinator.owner), self.attempt)
    }

    /// The exact committed generation against which preparation must run.
    pub(crate) fn base(&self) -> &RuntimeView {
        &self.base
    }

    /// Completes a semantic no-op without allocating or publishing a generation.
    pub(crate) fn finish_noop(self) -> RuntimeView {
        self.base
    }

    /// Validates a fully prepared candidate against this attempt's exact base and stamps
    /// it for terminal publication.
    ///
    /// All stale/wrong-base rejection happens here, before any caller side effect. The
    /// returned token owns the sole-writer lease, the attempt identity, the exact base
    /// generation, and the fully committed candidate; from here publication is infallible.
    pub(crate) fn prepare_commit(
        self,
        candidate: PreparedRuntimeGeneration,
    ) -> Result<PreparedRuntimeCommit, StaleRuntimeProposal> {
        if candidate.graph.generation() != self.base.id() {
            return Err(StaleRuntimeProposal {
                attempt: self.attempt,
                base: candidate.graph.generation(),
                current: self.base.id(),
            });
        }

        let next = RuntimeGenerationId::new(
            self.base
                .id()
                .get()
                .checked_add(1)
                .expect("runtime generation sequence exhausted"),
        );
        let RuntimeTransition {
            coordinator,
            writer,
            attempt,
            base,
        } = self;
        let (candidate, commit_actions) =
            candidate.commit(Arc::clone(&coordinator.owner), next, &coordinator.displaced);
        let candidate = Arc::new(candidate);

        Ok(PreparedRuntimeCommit {
            coordinator,
            writer,
            attempt,
            base: base.generation,
            candidate,
            commit_actions,
        })
    }

    /// Atomically publishes a fully prepared generation.
    ///
    /// Compatibility wrapper around [`RuntimeTransition::prepare_commit`] and
    /// [`PreparedRuntimeCommit::commit_with`] with an empty legacy-config callback.
    ///
    /// All validation, hooks, and construction must finish before calling this method.
    #[doc(hidden)]
    #[allow(dead_code, reason = "used by the next component-strategy integration")]
    pub(crate) fn publish(
        self,
        candidate: PreparedRuntimeGeneration,
    ) -> Result<RuntimeView, StaleRuntimeProposal> {
        self.prepare_commit(candidate)
            .map(|commit| commit.commit_with(|| ()))
    }
}
