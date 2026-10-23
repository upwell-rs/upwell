//! Structural diffing of two effective graphs and the dependency contract it compares.

use std::collections::BTreeSet;

use super::{GraphDiff, NodeChange, NodeChangeKind};
use crate::transition::{DependencyDemand, EffectiveGraph, EffectiveTarget};

/// The diff of a graph against one sharing its data: every node unchanged, so the
/// planner has no structural or binding change to look for.
pub(super) fn unchanged_diff(graph: &EffectiveGraph) -> GraphDiff {
    GraphDiff {
        nodes: graph
            .nodes
            .keys()
            .map(|component| NodeChange {
                component,
                kinds: Box::new([NodeChangeKind::Unchanged]),
            })
            .collect(),
    }
}

pub(super) fn diff(active: &EffectiveGraph, candidate: &EffectiveGraph) -> GraphDiff {
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

fn dependency_contracts(dependencies: &[DependencyDemand]) -> Vec<DependencyContract<'_>> {
    dependencies.iter().map(demand_contract).collect()
}

fn dependency_targets(dependencies: &[DependencyDemand]) -> Vec<&[EffectiveTarget]> {
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

pub(super) fn demand_contract(demand: &DependencyDemand) -> DependencyContract<'_> {
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
