//! Immutable effective dependency graphs and pure transition planning.

mod graph;
mod node;
mod plan;
mod retirement;

pub use graph::EffectiveGraph;
pub use node::{
    DependencyDemand, DependencyDemandId, EffectiveNode, EffectiveNodeRole, EffectiveTarget,
    FactoryIdentity,
};

pub use plan::{
    BindingTransition, GraphDiff, NodeAction, NodeChange, NodeChangeKind, PlannedNode, ReasonKind,
    StaleGraphCandidate, TransitionPlan, TransitionReason,
};

#[cfg(test)]
mod tests;
