use std::sync::Arc;

use upwell_core::RuntimeGenerationId;
use upwell_di::{ComponentDescriptor, EffectiveGraph, ScopeContainer, ScopeRegistry};
use upwell_hooks::HookManager;

use super::super::RuntimeScopePlan;
use super::{AppConditionState, RuntimeGeneration};
use crate::transition::HeldLiveCells;

/// One pinned, internally consistent view of a committed runtime generation.
#[derive(Clone)]
pub struct RuntimeView {
    pub(super) generation: Arc<RuntimeGeneration>,
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

    /// The live cells this generation's retained consumers hold.
    pub(crate) fn held_live_cells(&self) -> &HeldLiveCells {
        &self.generation.held_live_cells
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
    pub(crate) fn condition(&self) -> &Arc<AppConditionState> {
        &self.generation.condition
    }

    /// The hook manager pinned by this generation. Hook receivers resolve through this
    /// generation's root, so an old pinned view keeps invoking through its own manager.
    pub fn hooks(&self) -> &HookManager {
        &self.generation.hooks
    }
}
