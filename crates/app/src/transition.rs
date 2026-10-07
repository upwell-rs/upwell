use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

use futures::FutureExt;
use upwell_core::{ResolverSet, RuntimeGenerationId, ScopeId, StaticScope};
use upwell_di::{
    BoxedComponent, ComponentDescriptor, EffectiveGraph, EffectiveNodeRole, NodeAction,
    PlannedNode, ProviderSelectionModel, ScopeContainer, ScopeRegistry, TransitionPlan,
};

use crate::runtime::RuntimeScopePlan;
use crate::scope::{PreparedScopeTopology, ScopePlan, SeedDestination};
use crate::{AppConditionEvaluation, AppRegistry};

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

/// A component's explicit choice for satisfying one planned singleton transition.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentTransitionStrategy {
    /// Preserve the current instance without invoking user construction code.
    Retain,
    /// Invoke the candidate descriptor's ordinary selected factory.
    Reconstruct,
}

/// One component-owned strategy decision for a transition attempt.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentTransitionDecision {
    pub component: &'static str,
    pub strategy: ComponentTransitionStrategy,
}

/// Why an otherwise valid graph transition requires process restart in the v1 runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RestartReason {
    MissingDecision,
    DuplicateDecision,
    UnknownDecision,
    StaleRetention,
    GenerationBoundDependency,
    ManualInstanceUnsupported,
    HandleRetentionUnsupported,
    LiveRebindUnsupported,
    RemovalUnsupported,
    NonSingleton,
    FactoryUnavailable,
}

impl fmt::Display for RestartReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::MissingDecision => "the component did not choose a transition strategy",
            Self::DuplicateDecision => "the component produced conflicting transition decisions",
            Self::UnknownDecision => "the decision does not belong to an affected component",
            Self::StaleRetention => "retaining the component would preserve a stale dependency",
            Self::GenerationBoundDependency => {
                "the component depends on generation-bound runtime state"
            }
            Self::ManualInstanceUnsupported => "the pre-built component has no transition contract",
            Self::HandleRetentionUnsupported => {
                "the component handle cannot be isolated across runtime generations"
            }
            Self::LiveRebindUnsupported => {
                "retained live dependency rebinding is not integrated yet"
            }
            Self::RemovalUnsupported => "component removal is not integrated yet",
            Self::NonSingleton => "runtime transitions are singleton-only",
            Self::FactoryUnavailable => "the candidate component has no ordinary factory",
        };

        formatter.write_str(reason)
    }
}

/// A validated transition that cannot be applied safely by the current runtime.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("component '{component}' requires restart: {reason}")]
pub struct RestartRequired {
    pub component: &'static str,
    pub required: Option<NodeAction>,
    pub reason: RestartReason,
}

/// Explicit execution choices validated against the graph planner's minimum requirements.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTransitionPlan {
    candidate_identity: Arc<()>,
    active_identity: Arc<()>,
    structural: TransitionPlan,
    decisions: Box<[ComponentTransitionDecision]>,
    construction_order: Box<[&'static str]>,
}

impl ResolvedTransitionPlan {
    pub fn structural(&self) -> &TransitionPlan {
        &self.structural
    }

    pub fn decisions(&self) -> &[ComponentTransitionDecision] {
        &self.decisions
    }

    pub fn construction_order(&self) -> &[&'static str] {
        &self.construction_order
    }

    /// Constructs the isolated candidate singleton root selected by this resolved plan.
    ///
    /// Retained instances must be supplied explicitly; prefer
    /// [`build_candidate_root_from_active`](Self::build_candidate_root_from_active),
    /// which derives them from the pinned active root.
    #[doc(hidden)]
    pub async fn build_candidate_root(
        &self,
        candidate: &CandidateGraph,
        retained: Vec<BoxedComponent>,
        externals: ResolverSet,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        if !Arc::ptr_eq(&self.candidate_identity, &candidate.identity) {
            return Err(upwell_di::StaleGraphCandidate {
                active: self.structural.base_generation,
                candidate: candidate.graph.generation(),
            }
            .into());
        }

        validate_retained(candidate, &self.decisions, &retained)?;

        std::panic::AssertUnwindSafe(async {
            let transient = candidate
                .components
                .iter()
                .filter(|component| component.scope.is_transient())
                .map(|component| (component.ty.type_id, *component))
                .collect();
            let components = candidate
                .components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect();
            let scopes = Arc::new(ScopeRegistry::from_selection_model(
                transient,
                components,
                Arc::clone(candidate.provider_selection()),
            )?);
            let root = ScopeContainer::build_candidate_root(
                &self.construction_order,
                retained,
                externals,
                Arc::clone(&scopes),
            )
            .await?;

            Ok((root, scopes))
        })
        .catch_unwind()
        .await
        .map_err(|_| crate::Error::CandidatePreparationPanicked)?
    }

    /// Constructs the isolated candidate singleton root, deriving every retained
    /// instance directly from the pinned active root.
    ///
    /// Each `Retain` decision is satisfied by snapshotting the component out of the
    /// active root's local store through its typed generation-snapshot adapter. A raw
    /// manual descriptor carries no adapter, so retaining it is rejected with
    /// [`RestartReason::ManualInstanceUnsupported`]. A typed handle that declines
    /// generation-local snapshots returns [`RestartReason::HandleRetentionUnsupported`];
    /// missing storage, panic, and type mismatch surface as DI errors.
    #[doc(hidden)]
    pub async fn build_candidate_root_from_active(
        &self,
        candidate: &CandidateGraph,
        active: &crate::RuntimeView,
        externals: ResolverSet,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        self.build_candidate_root_from_active_with_overrides(
            candidate,
            active,
            externals,
            Vec::new(),
        )
        .await
    }

    /// Constructs the isolated candidate singleton root like
    /// [`build_candidate_root_from_active`](Self::build_candidate_root_from_active),
    /// replacing select snapshot-derived seeds with caller-supplied generation
    /// overrides.
    ///
    /// `generation_overrides` replaces the snapshot-derived seed for a retained
    /// framework singleton that must be fresh in every generation (the
    /// `HookManager`). Each override must be unique and must target a component this
    /// plan retains; every other retained singleton still comes from the snapshot
    /// path, and the merged seed set must still satisfy the plan's complete retain
    /// dispositions.
    #[doc(hidden)]
    pub async fn build_candidate_root_from_active_with_overrides(
        &self,
        candidate: &CandidateGraph,
        active: &crate::RuntimeView,
        externals: ResolverSet,
        generation_overrides: Vec<BoxedComponent>,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        if !Arc::ptr_eq(&self.active_identity, active.effective_graph().identity()) {
            return Err(upwell_di::StaleGraphCandidate {
                active: self.structural.base_generation,
                candidate: active.id(),
            }
            .into());
        }

        validate_generation_overrides(candidate, &self.decisions, &generation_overrides)?;

        let overridden = generation_overrides
            .iter()
            .map(|seed| seed.ty.type_id)
            .collect::<std::collections::BTreeSet<_>>();
        let mut retained =
            self.derive_retained_from_active(candidate, active.root(), &overridden)?;

        retained.extend(generation_overrides);

        self.build_candidate_root(candidate, retained, externals)
            .await
    }

    /// Snapshots one stored representation per `Retain` decision out of the active root.
    ///
    /// Overridden types are skipped: their seed is supplied by the caller instead of
    /// the snapshot adapter, so snapshot safety is never bypassed for them.
    fn derive_retained_from_active(
        &self,
        candidate: &CandidateGraph,
        active_root: &ScopeContainer,
        overridden: &std::collections::BTreeSet<std::any::TypeId>,
    ) -> crate::Result<Vec<BoxedComponent>> {
        let mut retained = Vec::new();

        for decision in self
            .decisions
            .iter()
            .filter(|decision| decision.strategy == ComponentTransitionStrategy::Retain)
        {
            let Some(descriptor) = candidate
                .components
                .iter()
                .find(|component| component.id == decision.component)
            else {
                continue;
            };

            if overridden.contains(&descriptor.ty.type_id) {
                continue;
            }

            match active_root.snapshot_singleton(*descriptor) {
                Ok(component) => retained.push(component),
                Err(upwell_di::Error::SnapshotUnavailable { .. }) => {
                    return Err(RestartRequired {
                        component: decision.component,
                        required: Some(NodeAction::Retain),
                        reason: RestartReason::ManualInstanceUnsupported,
                    }
                    .into());
                }
                Err(upwell_di::Error::SnapshotHandleUnsupported { .. }) => {
                    return Err(RestartRequired {
                        component: decision.component,
                        required: Some(NodeAction::Retain),
                        reason: RestartReason::HandleRetentionUnsupported,
                    }
                    .into());
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(retained)
    }
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

    /// Validates explicit component choices against the planner's minimum structural work.
    #[doc(hidden)]
    pub fn resolve_transition(
        &self,
        active: &EffectiveGraph,
        requested: impl IntoIterator<Item = ComponentTransitionDecision>,
    ) -> crate::Result<ResolvedTransitionPlan> {
        let requested = requested.into_iter().collect::<Vec<_>>();

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let structural = active.plan_transition(&self.graph)?;

            self.resolve_transition_plan(active, structural, requested, false)
        }))
        .map_err(|_| crate::Error::CandidatePreparationPanicked)?
    }

    /// Resolves the deterministic v1 transition defaults used only by the transactional
    /// config reload while forcing runtime-invalidated component roots to reconstruct:
    /// structural Retain nodes retain, Add/Replace/RebindLive nodes reconstruct (factory
    /// availability is validated by the shared decision checks), and Remove nodes retire
    /// safely by absence from the candidate root — they carry no decision and require no
    /// candidate seed. An invalidated factoryless or non-singleton node fails resolution
    /// with [`RestartRequired`] before any construction runs.
    pub(crate) fn resolve_runtime_transition(
        &self,
        active: &EffectiveGraph,
        invalidated_roots: &BTreeSet<&'static str>,
    ) -> crate::Result<ResolvedTransitionPlan> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let structural =
                active.plan_transition_with_invalidations(&self.graph, invalidated_roots)?;

            let requested = structural
                .nodes
                .iter()
                .filter(|node| node.action != NodeAction::Remove)
                .map(|node| ComponentTransitionDecision {
                    component: node.component,
                    strategy: match node.action {
                        NodeAction::Retain => ComponentTransitionStrategy::Retain,

                        _ => ComponentTransitionStrategy::Reconstruct,
                    },
                })
                .collect::<Vec<_>>();

            self.resolve_transition_plan(active, structural, requested, true)
        }))
        .map_err(|_| crate::Error::CandidatePreparationPanicked)?
    }

    /// Completes a resolved plan from an already computed structural plan and explicit
    /// decisions.
    ///
    /// `retire_removed_by_absence` selects the runtime reload's default policy: a Remove
    /// node with no decision is a safe retirement (the component is simply absent from
    /// the candidate root) instead of a restart requirement. The explicit
    /// [`resolve_transition`](Self::resolve_transition) path keeps rejecting removals.
    fn resolve_transition_plan(
        &self,
        active: &EffectiveGraph,
        structural: TransitionPlan,
        requested: Vec<ComponentTransitionDecision>,
        retire_removed_by_absence: bool,
    ) -> crate::Result<ResolvedTransitionPlan> {
        let mut decisions = BTreeMap::new();

        for decision in requested {
            if decisions
                .insert(decision.component, decision.strategy)
                .is_some()
            {
                return Err(RestartRequired {
                    component: decision.component,
                    required: structural
                        .nodes
                        .iter()
                        .find(|node| node.component == decision.component)
                        .map(|node| node.action),
                    reason: RestartReason::DuplicateDecision,
                }
                .into());
            }
        }

        if let Some(component) = decisions
            .keys()
            .find(|component| {
                !structural
                    .nodes
                    .iter()
                    .any(|node| node.component == **component)
            })
            .copied()
        {
            return Err(RestartRequired {
                component,
                required: None,
                reason: RestartReason::UnknownDecision,
            }
            .into());
        }
        let mut resolved = Vec::with_capacity(structural.nodes.len());

        for node in &structural.nodes {
            let strategy = decisions.get(node.component).copied();

            if retire_removed_by_absence && strategy.is_none() && node.action == NodeAction::Remove
            {
                continue;
            }

            validate_decision(self, node, strategy)?;

            if let Some(strategy) = strategy {
                resolved.push(ComponentTransitionDecision {
                    component: node.component,
                    strategy,
                });
            }
        }

        for component in self
            .components
            .iter()
            .filter(|component| component.scope.id() == upwell_core::Singleton::ID)
            .filter(|component| component.id != upwell_di::ROOT_RESOLVER_ID)
        {
            if resolved
                .iter()
                .any(|decision| decision.component == component.id)
            {
                continue;
            }

            if depends_on_root_resolver(&self.graph, component.id) {
                return Err(RestartRequired {
                    component: component.id,
                    required: Some(NodeAction::Replace),
                    reason: RestartReason::GenerationBoundDependency,
                }
                .into());
            }

            resolved.push(ComponentTransitionDecision {
                component: component.id,
                strategy: ComponentTransitionStrategy::Retain,
            });
        }

        if let Some(decision) = resolved.iter().find(|decision| {
            decision.strategy == ComponentTransitionStrategy::Retain
                && depends_on_root_resolver(&self.graph, decision.component)
        }) {
            return Err(RestartRequired {
                component: decision.component,
                required: Some(NodeAction::Replace),
                reason: RestartReason::GenerationBoundDependency,
            }
            .into());
        }

        let reconstructing = resolved
            .iter()
            .filter(|decision| decision.strategy == ComponentTransitionStrategy::Reconstruct)
            .map(|decision| decision.component)
            .collect::<std::collections::BTreeSet<_>>();
        let construction_order = self
            .graph
            .construction_order()
            .iter()
            .filter(|component| reconstructing.contains(**component))
            .copied()
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(ResolvedTransitionPlan {
            candidate_identity: Arc::clone(&self.identity),
            active_identity: Arc::clone(active.identity()),
            structural,
            decisions: resolved.into_boxed_slice(),
            construction_order,
        })
    }
}

fn validate_decision(
    candidate: &CandidateGraph,
    node: &PlannedNode,
    strategy: Option<ComponentTransitionStrategy>,
) -> crate::Result<()> {
    if node.role != EffectiveNodeRole::Singleton {
        return restart(node, RestartReason::NonSingleton);
    }

    match node.action {
        NodeAction::Remove => restart(node, RestartReason::RemovalUnsupported),
        NodeAction::RebindLive if strategy == Some(ComponentTransitionStrategy::Retain) => {
            restart(node, RestartReason::LiveRebindUnsupported)
        }
        NodeAction::Replace if strategy == Some(ComponentTransitionStrategy::Retain) => {
            restart(node, RestartReason::StaleRetention)
        }
        NodeAction::Retain if strategy == Some(ComponentTransitionStrategy::Retain) => Ok(()),
        NodeAction::Add | NodeAction::Replace | NodeAction::RebindLive
            if strategy == Some(ComponentTransitionStrategy::Reconstruct) =>
        {
            let component = candidate
                .components
                .iter()
                .find(|component| component.id == node.component);
            let has_factory = match component {
                Some(component) => component.effective_factory()?.is_some(),
                None => false,
            };

            if has_factory {
                Ok(())
            } else {
                restart(node, RestartReason::FactoryUnavailable)
            }
        }
        _ => restart(node, RestartReason::MissingDecision),
    }
}

fn restart<T>(node: &PlannedNode, reason: RestartReason) -> crate::Result<T> {
    Err(RestartRequired {
        component: node.component,
        required: Some(node.action),
        reason,
    }
    .into())
}

/// Validates generation override seeds against the resolved plan's retain dispositions.
///
/// An override replaces the snapshot-derived retained instance for exactly one retained
/// framework singleton. Overrides must be unique and must target a component the plan
/// retains, so reconstruction and snapshot safety are never bypassed; the merged seed
/// set is still completeness-checked by [`validate_retained`] and the DI candidate
/// input validation.
fn validate_generation_overrides(
    candidate: &CandidateGraph,
    decisions: &[ComponentTransitionDecision],
    overrides: &[BoxedComponent],
) -> crate::Result<()> {
    let mut seen = std::collections::BTreeSet::new();

    for seed in overrides {
        let type_name = seed.ty.name;

        if !seen.insert(seed.ty.type_id) {
            return Err(crate::Error::DuplicateGenerationOverride { type_name });
        }

        let retained = decisions
            .iter()
            .filter(|decision| decision.strategy == ComponentTransitionStrategy::Retain)
            .filter_map(|decision| {
                candidate
                    .components
                    .iter()
                    .find(|component| component.id == decision.component)
            })
            .any(|component| component.ty.type_id == seed.ty.type_id);

        if !retained {
            return Err(crate::Error::InvalidGenerationOverride { type_name });
        }
    }

    Ok(())
}

fn depends_on_root_resolver(graph: &EffectiveGraph, component: &str) -> bool {
    graph.node(component).is_some_and(|node| {
        node.dependencies.iter().any(|dependency| {
            dependency
                .targets
                .iter()
                .copied()
                .any(|target| target.component_id() == Some(upwell_di::ROOT_RESOLVER_ID))
        })
    })
}

fn validate_retained(
    candidate: &CandidateGraph,
    decisions: &[ComponentTransitionDecision],
    retained: &[BoxedComponent],
) -> crate::Result<()> {
    let retained = retained
        .iter()
        .map(|component| component.ty.type_id)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = decisions
        .iter()
        .filter(|decision| decision.strategy == ComponentTransitionStrategy::Retain)
        .filter_map(|decision| {
            candidate
                .components
                .iter()
                .find(|component| component.id == decision.component)
                .map(|component| component.ty.type_id)
        })
        .collect::<std::collections::BTreeSet<_>>();

    if retained == expected {
        return Ok(());
    }

    let component = decisions
        .iter()
        .find(|decision| {
            candidate
                .components
                .iter()
                .find(|component| component.id == decision.component)
                .is_some_and(|component| {
                    retained.contains(&component.ty.type_id)
                        != (decision.strategy == ComponentTransitionStrategy::Retain)
                })
        })
        .map_or("<retained>", |decision| decision.component);

    Err(RestartRequired {
        component,
        required: Some(NodeAction::Retain),
        reason: RestartReason::MissingDecision,
    }
    .into())
}

#[cfg(test)]
mod tests;
