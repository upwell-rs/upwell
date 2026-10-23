use std::collections::BTreeSet;

use upwell_core::RuntimeGenerationId;

use super::{DependencyDemand, DependencyDemandId, EffectiveGraph, EffectiveNodeRole};

mod diff;
mod planner;

use planner::Planner;

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
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum NodeAction {
    Retain,
    RebindLive,
    Replace,
    Add,
    Remove,
}

/// Stable category explaining why an action is required.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
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
    RuntimeInvalidated,
}

/// Canonical shortest reason path from a direct change to an affected consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
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
    /// Whether the runtime can republish the consumer's held live cell in place,
    /// keeping the consumer instance. `false` means the consumer must be constructed
    /// again for the binding to observe its new target.
    pub in_place: bool,
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
    /// Returns whether every live binding `component` must republish can be rebound in
    /// place, so the component may keep its instance. `false` when it has none.
    pub fn rebinds_in_place(&self, component: &str) -> bool {
        let mut bindings = self
            .bindings
            .iter()
            .filter(|binding| binding.dependency.consumer == component)
            .peekable();

        bindings.peek().is_some() && bindings.all(|binding| binding.in_place)
    }

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

/// How the planner closes over live consumers whose binding must be republished.
#[derive(Clone, Copy)]
pub(super) enum LiveClosure<'a> {
    /// Ordinary DI planning: a live rebind stays local to the rebind itself.
    Local,
    /// Runtime planning: a binding rebinds in place when its demand shape allows it and
    /// the predicate accepts its active and candidate demands; every other live consumer
    /// is reconstructed and propagates to its own dependents like a replaced instance.
    Runtime(&'a dyn Fn(&DependencyDemand, &DependencyDemand) -> bool),
}

impl LiveClosure<'_> {
    fn in_place(self, before: &DependencyDemand, after: &DependencyDemand) -> bool {
        let rebindable = before.rebindable() && after.rebindable();

        match self {
            Self::Local => rebindable,
            Self::Runtime(accepts) => rebindable && accepts(before, after),
        }
    }

    fn propagates(self, in_place: bool) -> bool {
        matches!(self, Self::Runtime(_)) && !in_place
    }
}

/// Plans the transition requirements between two validated graphs.
///
/// `live` selects the planning mode. Runtime planning reconstructs every live consumer
/// it cannot rebind in place; such a consumer's own dependents must therefore propagate
/// exactly like replaced instances — even when `invalidated_roots` is empty, because a
/// pure provider/factory change can start the closure on its own. A consumer rebound in
/// place keeps its instance, so its dependents are unaffected. Ordinary planning keeps
/// live rebinding local to the rebind itself.
pub(super) fn plan_with_invalidations(
    active: &EffectiveGraph,
    candidate: &EffectiveGraph,
    invalidated_roots: &BTreeSet<&'static str>,
    live: LiveClosure<'_>,
) -> Result<TransitionPlan, StaleGraphCandidate> {
    if active.generation != candidate.generation {
        return Err(StaleGraphCandidate {
            active: active.generation,
            candidate: candidate.generation,
        });
    }

    let shared = active.shares_data_with(candidate);
    let diff = if shared {
        diff::unchanged_diff(candidate)
    } else {
        diff::diff(active, candidate)
    };
    let mut planner = Planner::new(active, candidate, live);

    if !shared {
        for change in &diff.nodes {
            planner.direct_change(change);
        }
    }

    planner.invalidate_roots(invalidated_roots);
    planner.propagate();

    Ok(planner.into_plan(diff))
}

pub(super) fn is_live(demand: &DependencyDemand) -> bool {
    demand.observation == upwell_core::DependencyObservation::Live
        && demand.cardinality == upwell_core::Cardinality::One
        && !demand.optional
        && !demand.dynamic
        && demand.targets.len() == 1
}
