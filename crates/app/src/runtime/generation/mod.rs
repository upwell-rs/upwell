use std::sync::Arc;

use upwell_core::{ResolverCtx, RuntimeGenerationId};
use upwell_di::{ComponentDescriptor, EffectiveGraph, ScopeContainer, ScopeRegistry};
use upwell_hooks::HookManager;

use super::RuntimeScopePlan;
use crate::registry::AppConditionEvaluation;
use crate::registry::AppRegistry;
use crate::transition::{CommitActions, HeldLiveCells};

mod commit;
mod coordinator;
mod release;
mod view;

use commit::RuntimePublication;
pub(crate) use commit::{PreparedRuntimeCommit, StaleRuntimeProposal};
pub(crate) use coordinator::{
    RuntimeTransition, RuntimeTransitionCoordinator, WeakTransitionCoordinator,
};

pub use release::DisplacedGeneration;
use release::{DisplacedGenerations, ReleaseGuard};
pub use view::RuntimeView;

/// Stable identity of one runtime transition attempt, including no-op and failed attempts.
///
/// Attempt IDs are allocated monotonically per runtime when an attempt acquires the
/// transition writer. Unlike [`RuntimeGenerationId`], every attempt consumes one, whether
/// it commits, is a no-op, or is rejected.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize), serde(transparent))]
pub struct TransitionAttemptId(u64);

impl TransitionAttemptId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the monotonic sequence value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TransitionAttemptId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
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

/// One committed runtime generation: the root and scope plan it serves resolutions from,
/// plus the graph, condition state, and hook manager committed with it.
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
    held_live_cells: HeldLiveCells,
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
    held_live_cells: HeldLiveCells,
    commit_actions: CommitActions,
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
            held_live_cells: HeldLiveCells::default(),
            commit_actions: CommitActions::default(),
        }
    }

    /// Attaches the live cells this generation's retained consumers hold, and the
    /// live rebinds and hot updates its terminal publication applies.
    pub(crate) fn with_commit_actions(
        self,
        held_live_cells: HeldLiveCells,
        commit_actions: CommitActions,
    ) -> Self {
        Self {
            held_live_cells,
            commit_actions,
            ..self
        }
    }

    /// Stamps this generation for publication, splitting off the actions the
    /// publication applies. Its root reports its final release to `displaced` once a later
    /// commit displaced it.
    fn commit(
        self,
        owner: Arc<()>,
        id: RuntimeGenerationId,
        displaced: &Arc<DisplacedGenerations>,
    ) -> (RuntimeGeneration, CommitActions) {
        let guard = ReleaseGuard::new(id, Arc::clone(displaced));
        let attached = self.root.retain_drop_guard(guard).is_ok();

        debug_assert!(attached, "a prepared root is committed only once");

        let generation = RuntimeGeneration {
            owner,
            id,
            root: self.root,
            scopes: self.scopes,
            scope_plan: self.scope_plan,
            resolved: self.resolved,
            graph: Arc::new(self.graph.into_committed_generation(id)),
            condition: self.condition,
            hooks: self.hooks,
            held_live_cells: self.held_live_cells,
        };

        (generation, self.commit_actions)
    }
}

#[cfg(test)]
mod tests;
