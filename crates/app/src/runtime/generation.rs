use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, OwnedMutexGuard};
use upwell_core::{ResolverCtx, RuntimeGenerationId};
use upwell_di::{ComponentDescriptor, EffectiveGraph, ScopeContainer, ScopeRegistry};
use upwell_hooks::HookManager;

use super::RuntimeScopePlan;
use crate::registry::AppConditionEvaluation;
use crate::registry::AppRegistry;

/// Stable identity of one runtime transition attempt, including no-op and failed attempts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TransitionAttemptId(u64);

impl TransitionAttemptId {
    /// Returns the monotonic sequence value.
    #[allow(dead_code, reason = "reserved for transition observability")]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The committed condition state of one runtime generation: the full immutable application
/// catalog plus the condition evaluation the generation was built from.
///
/// Publication stamps only the graph generation; this state is carried intact so a
/// transactional reload can re-evaluate conditions against the same catalog.
#[derive(Clone, Debug)]
pub struct AppConditionState {
    catalog: Arc<AppRegistry>,
    evaluation: AppConditionEvaluation,
}

impl AppConditionState {
    pub(crate) fn new(catalog: Arc<AppRegistry>, evaluation: AppConditionEvaluation) -> Self {
        Self {
            catalog,
            evaluation,
        }
    }

    /// The full immutable application catalog, including conditionally disabled descriptors.
    pub fn catalog(&self) -> &Arc<AppRegistry> {
        &self.catalog
    }

    /// The condition evaluation this generation was built from.
    pub fn evaluation(&self) -> &AppConditionEvaluation {
        &self.evaluation
    }
}

pub(crate) struct RuntimeGeneration {
    owner: Arc<()>,
    id: RuntimeGenerationId,
    root: Arc<ScopeContainer>,
    scopes: Arc<ScopeRegistry>,
    scope_plan: RuntimeScopePlan,
    resolved: Arc<[ComponentDescriptor]>,
    graph: Arc<EffectiveGraph>,
    condition: Arc<AppConditionState>,
    hooks: HookManager,
}

/// A complete prepared generation whose semantic ID is assigned only at commit.
pub(crate) struct PreparedRuntimeGeneration {
    root: Arc<ScopeContainer>,
    scopes: Arc<ScopeRegistry>,
    scope_plan: RuntimeScopePlan,
    resolved: Arc<[ComponentDescriptor]>,
    graph: EffectiveGraph,
    condition: Arc<AppConditionState>,
    hooks: HookManager,
}

impl PreparedRuntimeGeneration {
    pub(crate) fn new(
        root: Arc<ScopeContainer>,
        scopes: Arc<ScopeRegistry>,
        scope_plan: RuntimeScopePlan,
        resolved: Arc<[ComponentDescriptor]>,
        graph: EffectiveGraph,
        condition: Arc<AppConditionState>,
    ) -> Self {
        // The generation's manager is the exact HookManager seeded in this root, so hook
        // receivers resolve through this generation's root only. The weak reference never
        // keeps the root alive.
        let hooks = root
            .get::<HookManager>()
            .expect("prepared runtime root seeds the framework hook manager");
        let hook_ctx: Arc<dyn ResolverCtx + Send + Sync> = root.clone();
        hooks.attach(Arc::downgrade(&hook_ctx));

        Self {
            root,
            scopes,
            scope_plan,
            resolved,
            graph,
            condition,
            hooks,
        }
    }

    fn commit(self, owner: Arc<()>, id: RuntimeGenerationId) -> RuntimeGeneration {
        RuntimeGeneration {
            owner,
            id,
            root: self.root,
            scopes: self.scopes,
            scope_plan: self.scope_plan,
            resolved: self.resolved,
            graph: Arc::new(self.graph.into_committed_generation(id)),
            condition: self.condition,
            hooks: self.hooks,
        }
    }
}

#[derive(Clone)]
struct RuntimePublication {
    current: Arc<ArcSwap<RuntimeGeneration>>,
}

impl RuntimePublication {
    fn new(initial: RuntimeGeneration) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(initial)),
        }
    }

    fn pin(&self) -> RuntimeView {
        RuntimeView::from_generation(self.current.load_full())
    }

    fn publish(
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

/// Sole serializer and atomic publication owner for runtime-generation transitions.
#[derive(Clone)]
pub(crate) struct RuntimeTransitionCoordinator {
    owner: Arc<()>,
    publication: RuntimePublication,
    #[allow(dead_code, reason = "used by the reserved transition entry point")]
    writer: Arc<Mutex<()>>,
    #[allow(dead_code, reason = "used by the reserved transition entry point")]
    next_attempt: Arc<AtomicU64>,
}

impl RuntimeTransitionCoordinator {
    pub(crate) fn new(initial: PreparedRuntimeGeneration) -> Self {
        let owner = Arc::new(());
        let initial = initial.commit(Arc::clone(&owner), RuntimeGenerationId::INITIAL);
        let publication = RuntimePublication::new(initial);

        Self {
            owner,
            publication,
            writer: Arc::new(Mutex::new(())),
            next_attempt: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Pins the complete currently committed runtime generation.
    pub(crate) fn current(&self) -> RuntimeView {
        self.publication.pin()
    }

    pub(crate) fn owns(&self, generation: &Arc<RuntimeGeneration>) -> bool {
        Arc::ptr_eq(&self.owner, &generation.owner)
    }

    /// Starts one serialized transition attempt against the exact current generation.
    #[allow(
        dead_code,
        reason = "reserved for the next transition-strategy integration"
    )]
    pub(crate) async fn begin(&self) -> RuntimeTransition {
        let writer = Arc::clone(&self.writer).lock_owned().await;
        let attempt = TransitionAttemptId(self.next_attempt.fetch_add(1, Ordering::Relaxed));
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
    #[allow(dead_code, reason = "reserved for transition observability")]
    pub(crate) const fn attempt(&self) -> TransitionAttemptId {
        self.attempt
    }

    /// The exact committed generation against which preparation must run.
    #[allow(
        dead_code,
        reason = "reserved for the next transition-strategy integration"
    )]
    pub(crate) fn base(&self) -> &RuntimeView {
        &self.base
    }

    /// Completes a semantic no-op without allocating or publishing a generation.
    #[allow(
        dead_code,
        reason = "reserved for the next transition-strategy integration"
    )]
    pub(crate) fn finish_noop(self) -> RuntimeView {
        self.base
    }

    /// Validates a fully prepared candidate against this attempt's exact base and stamps
    /// it for terminal publication.
    ///
    /// All stale/wrong-base rejection happens here, before any caller side effect. The
    /// returned token owns the sole-writer lease, the attempt identity, the exact base
    /// generation, and the fully committed candidate; from here publication is infallible.
    #[allow(
        dead_code,
        reason = "used by the next transactional-config integration"
    )]
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
        let candidate = Arc::new(candidate.commit(Arc::clone(&coordinator.owner), next));

        Ok(PreparedRuntimeCommit {
            coordinator,
            writer,
            attempt,
            base: base.generation,
            candidate,
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

/// A validated, stamped candidate generation awaiting terminal publication.
///
/// Prepared by [`RuntimeTransition::prepare_commit`]; owns the sole-writer lease, the
/// attempt identity, the exact base generation, and the fully committed candidate.
pub(crate) struct PreparedRuntimeCommit {
    coordinator: RuntimeTransitionCoordinator,
    writer: OwnedMutexGuard<()>,
    attempt: TransitionAttemptId,
    base: Arc<RuntimeGeneration>,
    candidate: Arc<RuntimeGeneration>,
}

impl PreparedRuntimeCommit {
    /// Publishes the validated candidate, runs the synchronous legacy-config
    /// compatibility callback while still holding the writer, then releases the writer.
    ///
    /// Ordering: the already validated candidate generation becomes current first, the
    /// callback observes the new generation through the coordinator, and only then does
    /// the writer lease end. Publication cannot fail after `prepare_commit` accepted the
    /// candidate: the writer is the sole publication serializer, so a compare-and-swap
    /// failure would be an internal invariant violation, not a recoverable error.
    #[allow(
        dead_code,
        reason = "used by the next transactional-config integration"
    )]
    pub(crate) fn commit_with(self, commit_compat: impl FnOnce()) -> RuntimeView {
        let PreparedRuntimeCommit {
            coordinator,
            writer,
            attempt,
            base,
            candidate,
        } = self;

        let view = coordinator
            .publication
            .publish(attempt, &base, candidate)
            .expect("validated candidate must publish under the sole writer");

        commit_compat();

        drop(writer);

        view
    }
}

/// One pinned, internally consistent view of a committed runtime generation.
#[derive(Clone)]
pub struct RuntimeView {
    generation: Arc<RuntimeGeneration>,
}

impl std::fmt::Debug for RuntimeView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeView")
            .field("generation", &self.id())
            .finish_non_exhaustive()
    }
}

impl RuntimeView {
    pub(crate) fn from_generation(generation: Arc<RuntimeGeneration>) -> Self {
        Self { generation }
    }

    pub(crate) fn generation_state(&self) -> Arc<RuntimeGeneration> {
        Arc::clone(&self.generation)
    }

    /// The committed generation identity.
    pub fn id(&self) -> RuntimeGenerationId {
        self.generation.id
    }

    /// The root scope pinned by this generation.
    pub fn root(&self) -> &Arc<ScopeContainer> {
        &self.generation.root
    }

    pub(crate) fn scopes(&self) -> &Arc<ScopeRegistry> {
        &self.generation.scopes
    }

    pub(crate) fn scope_plan(&self) -> &RuntimeScopePlan {
        &self.generation.scope_plan
    }

    /// The resolved component descriptors committed in this generation.
    pub fn resolved_components(&self) -> &Arc<[ComponentDescriptor]> {
        &self.generation.resolved
    }

    /// The immutable effective dependency graph committed in this generation.
    pub fn effective_graph(&self) -> &Arc<EffectiveGraph> {
        &self.generation.graph
    }

    /// The committed condition state pinned by this generation.
    #[allow(
        dead_code,
        reason = "used by the next transactional reload integration"
    )]
    pub(crate) fn condition(&self) -> &Arc<AppConditionState> {
        &self.generation.condition
    }

    /// The hook manager pinned by this generation. Hook receivers resolve through this
    /// generation's root, so an old pinned view keeps invoking through its own manager.
    pub fn hooks(&self) -> &HookManager {
        &self.generation.hooks
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

#[cfg(test)]
mod tests;
