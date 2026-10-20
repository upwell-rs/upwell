//! Candidate effective graphs and the component-owned strategies that move a runtime
//! from its active generation to one of them.

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{RuntimeGenerationId, ScopeId};
use upwell_di::{ComponentDescriptor, EffectiveGraph, ProviderSelectionModel};

use crate::runtime::RuntimeScopePlan;
use crate::scope::{PreparedScopeTopology, ScopePlan, SeedDestination};
use crate::{AppConditionEvaluation, AppRegistry};

mod commit;
mod rebind;
mod resolve;
mod resolved;
mod strategy;

pub(crate) use commit::CommitActions;
pub(crate) use rebind::HeldLiveCells;
pub use resolved::ResolvedTransitionPlan;
pub use strategy::{
    ComponentTransitionDecision, ComponentTransitionStrategy, RestartReason, RestartRequired,
};

/// A validated candidate effective graph plus the plans used by future scope openings.
///
/// Preparing this value performs no construction and does not mutate active runtime state.
pub struct CandidateGraph {
    identity: Arc<()>,
    graph: EffectiveGraph,
    components: Box<[ComponentDescriptor]>,
    singleton_order: Box<[ComponentDescriptor]>,
    scope_orders: HashMap<ScopeId, Box<[ComponentDescriptor]>>,
    seed_destinations: HashMap<std::any::TypeId, SeedDestination>,
}

impl CandidateGraph {
    /// Evaluates application-specific graph invariants and freezes future scope plans.
    pub fn prepare(
        base_generation: RuntimeGenerationId,
        registry: &AppRegistry,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<Self> {
        Self::prepare_registry(
            base_generation,
            registry,
            registry.component_registry(),
            topology,
        )
    }

    /// Prepares a candidate from a complete condition evaluation while retaining the
    /// application's config-binding contract.
    pub fn prepare_evaluation(
        base_generation: RuntimeGenerationId,
        registry: &AppRegistry,
        evaluation: &AppConditionEvaluation,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<Self> {
        let component_registry = registry.component_registry();

        if !evaluation.evaluation().belongs_to(&component_registry)? {
            return Err(upwell_di::ConditionError::EvaluationCatalogMismatch.into());
        }

        if !evaluation.belongs_to(registry) {
            return Err(crate::Error::ConditionEvaluationApplicationMismatch);
        }

        Self::prepare_registry(
            base_generation,
            registry,
            evaluation.evaluation().eligible_registry().clone(),
            topology,
        )
    }

    fn prepare_registry(
        base_generation: RuntimeGenerationId,
        registry: &AppRegistry,
        component_registry: upwell_di::ComponentRegistry,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<Self> {
        let components = component_registry.resolved_components()?;
        registry.validate_configs(&components)?;

        let graph = EffectiveGraph::build(
            base_generation,
            &component_registry,
            |consumer, dependency| topology.is_reachable(&consumer, &dependency),
        )?;
        let selection = Arc::clone(graph.provider_selection());
        let scopes = ScopePlan::partition(&components, &selection, topology)?;
        let singleton_order = graph
            .construction_order()
            .iter()
            .filter_map(|id| components.iter().find(|component| component.id == *id))
            .copied()
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let scope_orders = scopes
            .orders
            .into_iter()
            .map(|(scope, order)| (scope, order.into_boxed_slice()))
            .collect();

        Ok(Self {
            identity: Arc::new(()),
            graph,
            components: components.into_boxed_slice(),
            singleton_order,
            scope_orders,
            seed_destinations: scopes.seed_destinations,
        })
    }

    pub fn graph(&self) -> &EffectiveGraph {
        &self.graph
    }

    pub fn components(&self) -> &[ComponentDescriptor] {
        &self.components
    }

    /// The candidate descriptor with stable ID `id`.
    pub fn component(&self, id: &str) -> Option<&ComponentDescriptor> {
        self.components.iter().find(|component| component.id == id)
    }

    pub fn provider_selection(&self) -> &Arc<ProviderSelectionModel> {
        self.graph.provider_selection()
    }

    pub fn singleton_order(&self) -> &[ComponentDescriptor] {
        &self.singleton_order
    }

    pub fn scope_order(&self, scope: &ScopeId) -> Option<&[ComponentDescriptor]> {
        self.scope_orders.get(scope).map(Box::as_ref)
    }

    pub fn seed_destination(&self, type_id: std::any::TypeId) -> Option<(ScopeId, &'static str)> {
        self.seed_destinations
            .get(&type_id)
            .map(|destination| (destination.scope, destination.type_name))
    }

    /// Consumes the candidate into the runtime-generation parts: the resolved descriptor
    /// set, the future-scope plan over `topology`, and the effective graph.
    /// Crate-internal: the transactional reload assembles a
    /// `PreparedRuntimeGeneration` from these without exposing the candidate's fields
    /// broadly.
    pub(crate) fn into_runtime_parts(
        self,
        topology: &Arc<PreparedScopeTopology>,
    ) -> (Arc<[ComponentDescriptor]>, RuntimeScopePlan, EffectiveGraph) {
        let scope_plan = RuntimeScopePlan::new(
            Arc::clone(topology),
            Arc::new(
                self.scope_orders
                    .into_iter()
                    .map(|(scope, order)| (scope, order.into_vec()))
                    .collect(),
            ),
            Arc::new(self.seed_destinations),
        );

        (Arc::from(self.components), scope_plan, self.graph)
    }
}

#[cfg(test)]
mod tests;
