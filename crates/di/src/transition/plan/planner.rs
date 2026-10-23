//! The transition planner: requirements from direct changes, then their closure over
//! dependents.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::diff::demand_contract;
use super::{
    BindingTransition, GraphDiff, LiveClosure, NodeAction, NodeChange, NodeChangeKind, PlannedNode,
    ReasonKind, TransitionPlan, TransitionReason, is_live,
};
use crate::transition::{DependencyDemand, DependencyDemandId, EffectiveGraph, EffectiveNodeRole};

/// The dependency path from a changed component to the consumer it affects.
type DemandPath = Vec<DependencyDemandId>;

/// The strongest action required of one component, with every reason requiring it.
struct Requirement {
    action: NodeAction,
    reasons: BTreeSet<(ReasonKind, DemandPath)>,
}

/// Accumulates the requirements of one transition between two graphs of the same
/// generation, and the queue of components whose instances change.
pub(super) struct Planner<'a> {
    active: &'a EffectiveGraph,
    candidate: &'a EffectiveGraph,
    live: LiveClosure<'a>,
    requirements: BTreeMap<&'static str, Requirement>,
    bindings: BTreeSet<BindingTransition>,
    changed: VecDeque<(&'static str, DemandPath)>,
}

impl<'a> Planner<'a> {
    pub(super) fn new(
        active: &'a EffectiveGraph,
        candidate: &'a EffectiveGraph,
        live: LiveClosure<'a>,
    ) -> Self {
        Self {
            active,
            candidate,
            live,
            requirements: BTreeMap::new(),
            bindings: BTreeSet::new(),
            changed: VecDeque::new(),
        }
    }

    /// Requires what one node's structural change implies, then compares its dependency
    /// demands when it exists in both graphs.
    pub(super) fn direct_change(&mut self, change: &NodeChange) {
        let direct = direct_requirements(&change.kinds);

        for &(action, kind) in &direct {
            self.require(change.component, action, kind, Vec::new());
        }

        if !direct.is_empty() {
            self.changed.push_back((change.component, Vec::new()));
        }

        let (Some(before), Some(after)) = (
            self.active.nodes.get(change.component),
            self.candidate.nodes.get(change.component),
        ) else {
            return;
        };

        self.compare_demands(before.dependencies.as_ref(), after.dependencies.as_ref());
    }

    /// Replaces every runtime-invalidated root present in both graphs.
    pub(super) fn invalidate_roots(&mut self, roots: &BTreeSet<&'static str>) {
        for &root in roots {
            if self.active.nodes.contains_key(root) && self.candidate.nodes.contains_key(root) {
                self.replace(root, ReasonKind::RuntimeInvalidated, Vec::new());
            }
        }
    }

    /// Closes the requirements over the dependents of every changed instance, once per
    /// changed component.
    pub(super) fn propagate(&mut self) {
        let mut propagated = BTreeSet::new();

        while let Some((changed, path)) = self.changed.pop_front() {
            if !propagated.insert(changed) {
                continue;
            }

            for demand in self.consumers_of(changed) {
                self.propagate_to(demand, &path);
            }
        }
    }

    /// The planned transition: every required node, the live bindings of the nodes that
    /// rebind, and the construction and retirement orders of the affected singletons.
    pub(super) fn into_plan(self, diff: GraphDiff) -> TransitionPlan {
        let Planner {
            active,
            candidate,
            requirements,
            mut bindings,
            ..
        } = self;
        let nodes = planned_nodes(active, candidate, requirements);
        let actions = nodes
            .iter()
            .map(|node| (node.component, node.action))
            .collect::<BTreeMap<_, _>>();

        bindings.retain(|binding| {
            actions.get(binding.dependency.consumer) == Some(&NodeAction::RebindLive)
        });

        TransitionPlan {
            base_generation: active.generation,
            diff,
            construction_order: construction_order(candidate, &actions),
            retirement_order: retirement_order(active, actions),
            nodes,
            bindings: bindings.into_iter().collect(),
        }
    }

    fn compare_demands(&mut self, before: &[DependencyDemand], after: &[DependencyDemand]) {
        let count = before.len().max(after.len());

        for ordinal in 0..count {
            let (Some(before), Some(after)) = (before.get(ordinal), after.get(ordinal)) else {
                let demand = before.get(ordinal).or_else(|| after.get(ordinal)).unwrap();

                self.replace(
                    demand.id.consumer,
                    ReasonKind::DependencyContractChanged,
                    vec![demand.id],
                );

                continue;
            };

            if demand_contract(before) != demand_contract(after) {
                self.replace(
                    after.id.consumer,
                    ReasonKind::DependencyContractChanged,
                    vec![after.id],
                );
            } else if before.targets != after.targets {
                if before.live_compatible_with(after) {
                    self.rebind(
                        after.id,
                        self.live.in_place(before, after),
                        ReasonKind::LiveTargetChanged,
                        vec![after.id],
                    );
                } else {
                    self.replace(
                        after.id.consumer,
                        ReasonKind::DependencyTargetChanged,
                        vec![after.id],
                    );
                }
            }
        }
    }

    fn consumers_of(&self, component: &'static str) -> BTreeSet<DependencyDemandId> {
        self.active
            .reverse
            .get(component)
            .into_iter()
            .flatten()
            .chain(self.candidate.reverse.get(component).into_iter().flatten())
            .copied()
            .collect()
    }

    /// Requires what a changed dependency implies for the consumer of `demand`: a live
    /// consumer rebinds, a fixed one is replaced.
    fn propagate_to(&mut self, demand: DependencyDemandId, path: &[DependencyDemandId]) {
        let active = self.active;
        let candidate = self.candidate;
        let active_demand = active
            .nodes
            .get(demand.consumer)
            .and_then(|node| node.dependencies.get(demand.ordinal));
        let candidate_demand = candidate
            .nodes
            .get(demand.consumer)
            .and_then(|node| node.dependencies.get(demand.ordinal));

        if candidate_demand.is_none() && active_demand.is_none() {
            return;
        }

        let mut dependent_path = path.to_vec();

        dependent_path.push(demand);

        if active_demand.is_none_or(is_live) && candidate_demand.is_none_or(is_live) {
            let in_place = active_demand
                .zip(candidate_demand)
                .is_some_and(|(before, after)| self.live.in_place(before, after));

            self.rebind(demand, in_place, ReasonKind::LiveDependent, dependent_path);
        } else if self.require(
            demand.consumer,
            NodeAction::Replace,
            ReasonKind::FixedDependent,
            dependent_path.clone(),
        ) {
            self.changed.push_back((demand.consumer, dependent_path));
        }
    }

    /// Requires `component`'s replacement and queues its dependents.
    fn replace(&mut self, component: &'static str, kind: ReasonKind, path: DemandPath) {
        self.require(component, NodeAction::Replace, kind, path.clone());
        self.changed.push_back((component, path));
    }

    /// Requires a live rebind of `dependency`'s consumer and records the binding; a
    /// consumer reconstructed rather than rebound in place queues its dependents.
    fn rebind(
        &mut self,
        dependency: DependencyDemandId,
        in_place: bool,
        kind: ReasonKind,
        path: DemandPath,
    ) {
        self.require(
            dependency.consumer,
            NodeAction::RebindLive,
            kind,
            path.clone(),
        );
        self.bindings.insert(BindingTransition {
            dependency,
            in_place,
        });

        if self.live.propagates(in_place) {
            self.changed.push_back((dependency.consumer, path));
        }
    }

    /// Records one reason for `component` and raises its action to `action` when that
    /// takes precedence. Returns whether the component's action changed.
    fn require(
        &mut self,
        component: &'static str,
        action: NodeAction,
        kind: ReasonKind,
        path: DemandPath,
    ) -> bool {
        let previous = self
            .requirements
            .get(component)
            .map(|requirement| requirement.action);
        let requirement = self
            .requirements
            .entry(component)
            .or_insert_with(|| Requirement {
                action,
                reasons: BTreeSet::new(),
            });

        if action_precedence(action) > action_precedence(requirement.action) {
            requirement.action = action;
        }

        requirement.reasons.insert((kind, path));

        previous.is_none_or(|previous| {
            action_precedence(previous) < action_precedence(requirement.action)
        })
    }
}

/// The actions and reasons a node's own structural change requires: an added or removed
/// node is added or removed, and a changed factory or identity replaces it.
fn direct_requirements(kinds: &[NodeChangeKind]) -> Vec<(NodeAction, ReasonKind)> {
    if kinds.contains(&NodeChangeKind::Added) {
        return vec![(NodeAction::Add, ReasonKind::Added)];
    }

    if kinds.contains(&NodeChangeKind::Removed) {
        return vec![(NodeAction::Remove, ReasonKind::Removed)];
    }

    [
        (NodeChangeKind::FactoryChanged, ReasonKind::FactoryChanged),
        (NodeChangeKind::IdentityChanged, ReasonKind::IdentityChanged),
    ]
    .into_iter()
    .filter(|(change, _)| kinds.contains(change))
    .map(|(_, reason)| (NodeAction::Replace, reason))
    .collect()
}

fn planned_nodes(
    active: &EffectiveGraph,
    candidate: &EffectiveGraph,
    requirements: BTreeMap<&'static str, Requirement>,
) -> Box<[PlannedNode]> {
    requirements
        .into_iter()
        .filter_map(|(component, requirement)| {
            let role = candidate
                .nodes
                .get(component)
                .or_else(|| active.nodes.get(component))?
                .role;
            let reasons = requirement
                .reasons
                .into_iter()
                .map(|(kind, path)| TransitionReason {
                    kind,
                    path: path.into_boxed_slice(),
                })
                .collect();

            Some(PlannedNode {
                component,
                role,
                action: requirement.action,
                reasons,
            })
        })
        .collect()
}

/// The candidate singletons the transition adds or replaces, in candidate construction
/// order.
fn construction_order(
    candidate: &EffectiveGraph,
    actions: &BTreeMap<&'static str, NodeAction>,
) -> Box<[&'static str]> {
    candidate
        .construction_order
        .iter()
        .filter(|component| {
            candidate
                .nodes
                .get(**component)
                .is_some_and(|node| node.role == EffectiveNodeRole::Singleton)
                && actions
                    .get(**component)
                    .is_some_and(|action| matches!(action, NodeAction::Add | NodeAction::Replace))
        })
        .copied()
        .collect()
}

/// The active singletons the transition removes or replaces, in retirement order.
fn retirement_order(
    active: &EffectiveGraph,
    actions: BTreeMap<&'static str, NodeAction>,
) -> Box<[&'static str]> {
    let retiring = actions
        .into_iter()
        .filter(|(component, action)| {
            active
                .nodes
                .get(component)
                .is_some_and(|node| node.role == EffectiveNodeRole::Singleton)
                && matches!(action, NodeAction::Remove | NodeAction::Replace)
        })
        .map(|(component, _)| component)
        .collect();

    active.retirement_order(&retiring).into_boxed_slice()
}

fn action_precedence(action: NodeAction) -> u8 {
    match action {
        NodeAction::Retain => 0,
        NodeAction::RebindLive => 1,
        NodeAction::Replace => 2,
        NodeAction::Add => 3,
        NodeAction::Remove => 4,
    }
}
