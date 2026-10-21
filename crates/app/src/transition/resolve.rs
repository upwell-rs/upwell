//! Resolution of explicit and runtime-default strategy decisions into a plan.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use upwell_core::StaticScope;
use upwell_di::{
    ComponentDescriptor, DependencyDemand, EffectiveGraph, EffectiveNodeRole, EffectiveTarget,
    NodeAction, PlannedNode, TransitionPlan,
};

use super::strategy::{updates_in_place, validate_decision};
use super::{
    CandidateGraph, ComponentTransitionDecision, ComponentTransitionStrategy,
    ResolvedTransitionPlan, RestartReason, RestartRequired,
};

impl CandidateGraph {
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
    /// structural Retain nodes retain, RebindLive nodes whose every changed live binding
    /// rebinds in place retain their instance, other Replace/RebindLive nodes use the
    /// most instance-preserving recipe their component declares (hot update, transition
    /// handler, then replacement factory), other Add/Replace/RebindLive nodes reconstruct
    /// (recipe availability is validated by the shared decision checks), and Remove nodes
    /// retire safely by absence from the candidate root — they carry no decision and
    /// require no candidate seed. An invalidated factoryless or non-singleton node fails
    /// resolution with [`RestartRequired`] before any construction runs.
    pub(crate) fn resolve_runtime_transition(
        &self,
        active: &EffectiveGraph,
        invalidated_roots: &BTreeSet<&'static str>,
    ) -> crate::Result<ResolvedTransitionPlan> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let structural = active.plan_runtime_transition(
                &self.graph,
                invalidated_roots,
                |before, after| self.rebinds_in_place(before, after),
            )?;

            let requested = structural
                .nodes
                .iter()
                .filter(|node| node.action != NodeAction::Remove)
                .map(|node| {
                    Ok(ComponentTransitionDecision {
                        component: node.component,
                        strategy: self.runtime_strategy(&structural, node)?,
                    })
                })
                .collect::<crate::Result<Vec<_>>>()?;

            self.resolve_transition_plan(active, structural, requested, true)
        }))
        .map_err(|_| crate::Error::CandidatePreparationPanicked)?
    }

    /// The component-declared default strategy for one structural runtime node.
    ///
    /// A `RebindLive` node keeps its instance when the plan can rebind every changed
    /// live binding in place. Otherwise a `Replace` or `RebindLive` node prefers, in
    /// order, the component's hot update (when its instance can update in place), its
    /// transition handler, and its replacement factory, and reconstructs when it declares
    /// none of them.
    fn runtime_strategy(
        &self,
        plan: &TransitionPlan,
        node: &PlannedNode,
    ) -> crate::Result<ComponentTransitionStrategy> {
        let strategy = match node.action {
            NodeAction::Retain => ComponentTransitionStrategy::Retain,

            NodeAction::RebindLive if plan.rebinds_in_place(node.component) => {
                ComponentTransitionStrategy::Retain
            }

            NodeAction::Replace | NodeAction::RebindLive => self.declared_strategy(node)?,
            _ => ComponentTransitionStrategy::Reconstruct,
        };

        Ok(strategy)
    }

    /// Whether the runtime can keep a live binding's consumer and republish the cell it
    /// holds: the consumer is [`retainable`](Self::retainable), and the live cell of
    /// every active and candidate target is reachable.
    ///
    /// A component target exposes its cell through its typed descriptor, whether or not
    /// the component itself is retained. A raw manual target, or one missing from the
    /// candidate, cannot be rebound, so its consumer reconstructs instead. Provider
    /// targets always expose their cell.
    fn rebinds_in_place(&self, before: &DependencyDemand, after: &DependencyDemand) -> bool {
        self.retainable(after.id.consumer)
            && before
                .targets
                .iter()
                .chain(after.targets.iter())
                .all(|target| self.exposes_live_cell(target))
    }

    fn exposes_live_cell(&self, target: &EffectiveTarget) -> bool {
        match target {
            EffectiveTarget::Component { component, .. } => self
                .component(component)
                .is_some_and(ComponentDescriptor::exposes_live_cell),
            EffectiveTarget::Provider { .. } => true,
            EffectiveTarget::Config { .. } | EffectiveTarget::Dynamic { .. } => false,
        }
    }

    /// Whether the runtime can carry `component`'s instance into the candidate: a
    /// singleton whose descriptor has not opted out of retention
    /// ([`is_retainable`](ComponentDescriptor::is_retainable)).
    fn retainable(&self, component: &str) -> bool {
        self.component(component).is_some_and(|descriptor| {
            descriptor.scope.id() == upwell_core::Singleton::ID && descriptor.is_retainable()
        })
    }

    /// The most instance-preserving strategy `node`'s component declares for a
    /// `Replace` or `RebindLive` node.
    pub(super) fn declared_strategy(
        &self,
        node: &PlannedNode,
    ) -> crate::Result<ComponentTransitionStrategy> {
        let Some(descriptor) = self.component(node.component) else {
            return Ok(ComponentTransitionStrategy::Reconstruct);
        };

        let strategy = if descriptor.hot_update_factory()?.is_some() && updates_in_place(node) {
            ComponentTransitionStrategy::HotUpdate
        } else if descriptor.transition_handler()?.is_some() {
            ComponentTransitionStrategy::Transition
        } else if descriptor.replacement_factory()?.is_some() {
            ComponentTransitionStrategy::Replace
        } else {
            ComponentTransitionStrategy::Reconstruct
        };

        Ok(strategy)
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

            validate_decision(self, &structural, node, strategy)?;

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
            decision.strategy.retains_instance()
                && depends_on_root_resolver(&self.graph, decision.component)
        }) {
            return Err(RestartRequired {
                component: decision.component,
                required: Some(NodeAction::Replace),
                reason: RestartReason::GenerationBoundDependency,
            }
            .into());
        }

        let constructing = resolved
            .iter()
            .filter(|decision| !decision.strategy.retains_instance())
            .map(|decision| decision.component)
            .collect::<std::collections::BTreeSet<_>>();
        let construction_order = self
            .graph
            .construction_order()
            .iter()
            .filter(|component| constructing.contains(**component))
            .copied()
            .collect::<Vec<_>>()
            .into_boxed_slice();

        let displaced = displaced(active, &structural, &resolved).into_boxed_slice();

        Ok(ResolvedTransitionPlan {
            candidate_identity: Arc::clone(&self.identity),
            active_identity: Arc::clone(active.identity()),
            structural,
            decisions: resolved.into_boxed_slice(),
            construction_order,
            displaced,
        })
    }
}

/// The active singletons `structural` displaces under `decisions`, in retirement order:
/// every removed singleton, and every replaced or rebound singleton whose decision does
/// not keep its instance. A rebound node that falls back to construction is displaced
/// like a replaced one.
fn displaced(
    active: &EffectiveGraph,
    structural: &TransitionPlan,
    decisions: &[ComponentTransitionDecision],
) -> Vec<&'static str> {
    let displaced = structural
        .nodes
        .iter()
        .filter(|node| node.role == EffectiveNodeRole::Singleton)
        .filter(|node| match node.action {
            NodeAction::Remove => true,
            NodeAction::Replace | NodeAction::RebindLive => decisions.iter().any(|decision| {
                decision.component == node.component && !decision.strategy.retains_instance()
            }),
            NodeAction::Retain | NodeAction::Add => false,
        })
        .map(|node| node.component)
        .collect();

    active.retirement_order(&displaced)
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
