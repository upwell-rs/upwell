use std::collections::{BTreeMap, BTreeSet, VecDeque};

use upwell_core::RuntimeGenerationId;

use super::{DependencyDemand, DependencyDemandId, EffectiveGraph, EffectiveNodeRole};

/// Direct structural classification of one component node.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NodeChangeKind {
    Added,
    Removed,
    FactoryChanged,
    IdentityChanged,
    DependenciesChanged,
    ProviderChanged,
    Unchanged,
}

/// One deterministic graph-diff entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeChange {
    pub component: &'static str,
    pub kinds: Box<[NodeChangeKind]>,
}

/// Complete node-by-node structural diff between active and candidate graphs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphDiff {
    pub nodes: Box<[NodeChange]>,
}

/// Minimum structural action required for one affected component.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NodeAction {
    Retain,
    RebindLive,
    Replace,
    Add,
    Remove,
}

/// Stable category explaining why an action is required.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReasonKind {
    Added,
    Removed,
    FactoryChanged,
    IdentityChanged,
    DependencyContractChanged,
    DependencyTargetChanged,
    LiveTargetChanged,
    FixedDependent,
    LiveDependent,
}

/// Canonical shortest reason path from a direct change to an affected consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionReason {
    pub kind: ReasonKind,
    pub path: Box<[DependencyDemandId]>,
}

/// One affected node and the minimum action the later execution layer must satisfy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedNode {
    pub component: &'static str,
    pub role: EffectiveNodeRole,
    pub action: NodeAction,
    pub reasons: Box<[TransitionReason]>,
}

/// One compatible live binding whose value must be republished.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BindingTransition {
    pub dependency: DependencyDemandId,
}

/// Pure deterministic transition requirements between two validated graphs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionPlan {
    pub base_generation: RuntimeGenerationId,
    pub diff: GraphDiff,
    pub nodes: Box<[PlannedNode]>,
    pub bindings: Box<[BindingTransition]>,
    pub construction_order: Box<[&'static str]>,
    pub retirement_order: Box<[&'static str]>,
}

impl TransitionPlan {
    /// Returns whether this plan requires no dependency-graph transition work.
    ///
    /// A wider runtime transaction may still publish config or other generation state.
    pub fn is_noop(&self) -> bool {
        self.nodes.is_empty()
            && self.bindings.is_empty()
            && self.construction_order.is_empty()
            && self.retirement_order.is_empty()
    }
}

/// Failure to compare graphs that do not share one active base generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "candidate graph is based on generation {candidate:?}, but active generation is {active:?}"
)]
pub struct StaleGraphCandidate {
    pub active: RuntimeGenerationId,
    pub candidate: RuntimeGenerationId,
}

#[derive(Clone)]
struct Requirement {
    action: NodeAction,
    reasons: BTreeSet<(ReasonKind, Vec<DependencyDemandId>)>,
}

pub(super) fn plan(
    active: &EffectiveGraph,
    candidate: &EffectiveGraph,
) -> Result<TransitionPlan, StaleGraphCandidate> {
    if active.generation != candidate.generation {
        return Err(StaleGraphCandidate {
            active: active.generation,
            candidate: candidate.generation,
        });
    }

    let diff = diff(active, candidate);
    let mut requirements = BTreeMap::new();
    let mut bindings = BTreeSet::new();
    let mut changed_instances = VecDeque::new();

    for change in &diff.nodes {
        let kinds = change.kinds.as_ref();
        let mut changed_directly = false;

        if kinds.contains(&NodeChangeKind::Added) {
            require(
                &mut requirements,
                change.component,
                NodeAction::Add,
                ReasonKind::Added,
                Vec::new(),
            );
            changed_directly = true;
        } else if kinds.contains(&NodeChangeKind::Removed) {
            require(
                &mut requirements,
                change.component,
                NodeAction::Remove,
                ReasonKind::Removed,
                Vec::new(),
            );
            changed_directly = true;
        } else {
            if kinds.contains(&NodeChangeKind::FactoryChanged) {
                require(
                    &mut requirements,
                    change.component,
                    NodeAction::Replace,
                    ReasonKind::FactoryChanged,
                    Vec::new(),
                );
                changed_directly = true;
            }

            if kinds.contains(&NodeChangeKind::IdentityChanged) {
                require(
                    &mut requirements,
                    change.component,
                    NodeAction::Replace,
                    ReasonKind::IdentityChanged,
                    Vec::new(),
                );
                changed_directly = true;
            }
        }

        if changed_directly {
            changed_instances.push_back((change.component, Vec::new()));
        }

        let (Some(before), Some(after)) = (
            active.nodes.get(change.component),
            candidate.nodes.get(change.component),
        ) else {
            continue;
        };

        compare_demands(
            before.dependencies.as_ref(),
            after.dependencies.as_ref(),
            &mut requirements,
            &mut bindings,
            &mut changed_instances,
        );
    }

    let mut propagated = BTreeSet::new();

    while let Some((changed, path)) = changed_instances.pop_front() {
        if !propagated.insert(changed) {
            continue;
        }

        let consumers = active
            .reverse
            .get(changed)
            .into_iter()
            .flatten()
            .chain(candidate.reverse.get(changed).into_iter().flatten())
            .copied()
            .collect::<BTreeSet<_>>();

        for demand_id in consumers {
            let active_demand = active
                .nodes
                .get(demand_id.consumer)
                .and_then(|node| node.dependencies.get(demand_id.ordinal));
            let candidate_demand = candidate
                .nodes
                .get(demand_id.consumer)
                .and_then(|node| node.dependencies.get(demand_id.ordinal));
            if candidate_demand.is_none() && active_demand.is_none() {
                continue;
            }
            let mut dependent_path = path.clone();
            dependent_path.push(demand_id);
            let live = active_demand.is_none_or(is_live) && candidate_demand.is_none_or(is_live);

            if live {
                require(
                    &mut requirements,
                    demand_id.consumer,
                    NodeAction::RebindLive,
                    ReasonKind::LiveDependent,
                    dependent_path,
                );
                bindings.insert(BindingTransition {
                    dependency: demand_id,
                });
            } else {
                let changed_action = require(
                    &mut requirements,
                    demand_id.consumer,
                    NodeAction::Replace,
                    ReasonKind::FixedDependent,
                    dependent_path.clone(),
                );

                if changed_action {
                    changed_instances.push_back((demand_id.consumer, dependent_path));
                }
            }
        }
    }

    let nodes = requirements
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
                .collect::<Vec<_>>()
                .into_boxed_slice();

            Some(PlannedNode {
                component,
                role,
                action: requirement.action,
                reasons,
            })
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    bindings.retain(|binding| {
        nodes.iter().any(|node| {
            node.component == binding.dependency.consumer && node.action == NodeAction::RebindLive
        })
    });
    let actions = nodes
        .iter()
        .map(|node| (node.component, node.action))
        .collect::<BTreeMap<_, _>>();
    let construction_order = candidate
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
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let retirement_actions = actions
        .into_iter()
        .filter(|(component, action)| {
            active
                .nodes
                .get(component)
                .is_some_and(|node| node.role == EffectiveNodeRole::Singleton)
                && matches!(action, NodeAction::Remove | NodeAction::Replace)
        })
        .collect();
    let retirement_order = retirement_order(active, &retirement_actions).into_boxed_slice();

    Ok(TransitionPlan {
        base_generation: active.generation,
        diff,
        nodes,
        bindings: bindings.into_iter().collect::<Vec<_>>().into_boxed_slice(),
        construction_order,
        retirement_order,
    })
}

fn retirement_order(
    active: &EffectiveGraph,
    executable: &BTreeMap<&'static str, NodeAction>,
) -> Vec<&'static str> {
    let mut remaining = executable
        .iter()
        .filter(|(_, action)| matches!(action, NodeAction::Remove | NodeAction::Replace))
        .map(|(component, _)| *component)
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(remaining.len());

    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|component| {
                !remaining.iter().any(|consumer| {
                    consumer != *component && fixed_targets(active, consumer).contains(*component)
                })
            })
            .copied()
            .collect::<Vec<_>>();

        if ready.is_empty() {
            order.extend(remaining.iter().copied());
            break;
        }

        for component in ready {
            remaining.remove(component);
            order.push(component);
        }
    }

    order
}

fn fixed_targets(graph: &EffectiveGraph, consumer: &str) -> BTreeSet<&'static str> {
    graph
        .nodes
        .get(consumer)
        .into_iter()
        .flat_map(|node| node.dependencies.iter())
        .filter(|dependency| !is_live(dependency))
        .flat_map(|dependency| dependency.targets.iter())
        .filter_map(|target| target.component_id())
        .collect()
}

fn diff(active: &EffectiveGraph, candidate: &EffectiveGraph) -> GraphDiff {
    let ids = active
        .nodes
        .keys()
        .chain(candidate.nodes.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let nodes = ids
        .into_iter()
        .map(|component| {
            let kinds = match (active.nodes.get(component), candidate.nodes.get(component)) {
                (None, Some(_)) => vec![NodeChangeKind::Added],
                (Some(_), None) => vec![NodeChangeKind::Removed],
                (Some(before), Some(after)) => {
                    let mut kinds = Vec::new();

                    if before.factory != after.factory {
                        kinds.push(NodeChangeKind::FactoryChanged);
                    }

                    if before.concrete_type != after.concrete_type
                        || before.scope != after.scope
                        || before.role != after.role
                    {
                        kinds.push(NodeChangeKind::IdentityChanged);
                    }

                    if dependency_contracts(before.dependencies.as_ref())
                        != dependency_contracts(after.dependencies.as_ref())
                    {
                        kinds.push(NodeChangeKind::DependenciesChanged);
                    }

                    if dependency_targets(before.dependencies.as_ref())
                        != dependency_targets(after.dependencies.as_ref())
                    {
                        kinds.push(NodeChangeKind::ProviderChanged);
                    }

                    if kinds.is_empty() {
                        kinds.push(NodeChangeKind::Unchanged);
                    }

                    kinds
                }
                (None, None) => unreachable!("union contains an existing component"),
            };

            NodeChange {
                component,
                kinds: kinds.into_boxed_slice(),
            }
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();

    GraphDiff { nodes }
}

fn compare_demands(
    before: &[DependencyDemand],
    after: &[DependencyDemand],
    requirements: &mut BTreeMap<&'static str, Requirement>,
    bindings: &mut BTreeSet<BindingTransition>,
    changed_instances: &mut VecDeque<(&'static str, Vec<DependencyDemandId>)>,
) {
    let count = before.len().max(after.len());

    for ordinal in 0..count {
        let (Some(before), Some(after)) = (before.get(ordinal), after.get(ordinal)) else {
            let demand = before.get(ordinal).or_else(|| after.get(ordinal)).unwrap();
            let path = vec![demand.id];

            require(
                requirements,
                demand.id.consumer,
                NodeAction::Replace,
                ReasonKind::DependencyContractChanged,
                path.clone(),
            );
            changed_instances.push_back((demand.id.consumer, path));

            continue;
        };

        if demand_contract(before) != demand_contract(after) {
            let path = vec![after.id];

            require(
                requirements,
                after.id.consumer,
                NodeAction::Replace,
                ReasonKind::DependencyContractChanged,
                path.clone(),
            );
            changed_instances.push_back((after.id.consumer, path));
        } else if before.targets != after.targets {
            let path = vec![after.id];

            if before.live_compatible_with(after) {
                require(
                    requirements,
                    after.id.consumer,
                    NodeAction::RebindLive,
                    ReasonKind::LiveTargetChanged,
                    path,
                );
                bindings.insert(BindingTransition {
                    dependency: after.id,
                });
            } else {
                require(
                    requirements,
                    after.id.consumer,
                    NodeAction::Replace,
                    ReasonKind::DependencyTargetChanged,
                    path.clone(),
                );
                changed_instances.push_back((after.id.consumer, path));
            }
        }
    }
}

fn require(
    requirements: &mut BTreeMap<&'static str, Requirement>,
    component: &'static str,
    action: NodeAction,
    kind: ReasonKind,
    path: Vec<DependencyDemandId>,
) -> bool {
    let previous = requirements
        .get(component)
        .map(|requirement| requirement.action);
    let requirement = requirements
        .entry(component)
        .or_insert_with(|| Requirement {
            action,
            reasons: BTreeSet::new(),
        });
    if action_precedence(action) > action_precedence(requirement.action) {
        requirement.action = action;
    }

    requirement.reasons.insert((kind, path));

    previous
        .is_none_or(|previous| action_precedence(previous) < action_precedence(requirement.action))
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

fn is_live(demand: &DependencyDemand) -> bool {
    demand.observation == upwell_core::DependencyObservation::Live
        && demand.cardinality == upwell_core::Cardinality::One
        && !demand.optional
        && !demand.dynamic
        && demand.targets.len() == 1
}

fn dependency_contracts(dependencies: &[DependencyDemand]) -> Vec<DependencyContract<'_>> {
    dependencies.iter().map(demand_contract).collect()
}

fn dependency_targets(dependencies: &[DependencyDemand]) -> Vec<&[super::EffectiveTarget]> {
    dependencies
        .iter()
        .map(|dependency| dependency.targets.as_ref())
        .collect()
}

type DependencyContract<'a> = (
    &'a str,
    upwell_core::Cardinality,
    bool,
    bool,
    Option<&'a str>,
    bool,
    upwell_core::ResolutionMode,
    upwell_core::DependencyObservation,
);

fn demand_contract(demand: &DependencyDemand) -> DependencyContract<'_> {
    (
        demand.requested_type,
        demand.cardinality,
        demand.optional,
        demand.dynamic,
        demand.qualifier,
        demand.config,
        demand.resolution,
        demand.observation,
    )
}
