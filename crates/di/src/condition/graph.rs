use std::collections::{BTreeMap, BTreeSet};

use upwell_core::{
    AvailabilityTarget, ConditionDescriptor, ConditionPredicate, ConditionPredicateKind,
    ConfigFactId, ProviderMappingId,
};

use super::{ConditionCatalog, ConditionError};

/// One fact whose change can invalidate a component's condition decision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConditionDependency {
    Config(ConfigFactId),
    Component(&'static str),
    Provider(ProviderMappingId),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AvailabilityEdge {
    pub from: &'static str,
    pub to: &'static str,
    pub condition_id: &'static str,
    pub predicate: ConditionPredicateKind,
    pub negated: bool,
}

impl ConditionCatalog {
    pub fn dependencies(
        &self,
        component_id: &str,
    ) -> Result<Vec<ConditionDependency>, ConditionError> {
        let component = self
            .components
            .get(component_id)
            .ok_or(ConditionError::UnknownComponentId(component_id.to_string()))?;
        let mut dependencies = BTreeSet::new();

        if let Some(root) = component.condition {
            collect_dependencies(root, &mut dependencies);
        }

        Ok(dependencies.into_iter().collect())
    }

    pub(super) fn validate_acyclic(&self) -> Result<(), ConditionError> {
        let graph = self.availability_graph();
        let mut completed = BTreeSet::new();

        for component in &self.component_order {
            let mut visiting = Vec::new();
            let mut active = BTreeSet::new();

            if let Some(cycle) = find_cycle(
                component,
                &graph,
                &mut visiting,
                &mut active,
                &mut completed,
            ) {
                crate::observability::availability_cycle(&cycle);

                return Err(ConditionError::AvailabilityCycle(cycle));
            }
        }

        Ok(())
    }

    fn availability_graph(&self) -> BTreeMap<&'static str, Vec<AvailabilityEdge>> {
        let mut graph = self
            .component_order
            .iter()
            .map(|id| (*id, Vec::new()))
            .collect::<BTreeMap<_, _>>();

        for (owner, component) in &self.components {
            if let Some(root) = component.condition {
                collect_edges(owner, root, false, &mut graph);
            }
        }

        for edges in graph.values_mut() {
            edges.sort();
            edges.dedup();

            for edge in edges.iter() {
                crate::observability::availability_edge(edge);
            }
        }

        graph
    }
}

fn collect_edges(
    owner: &'static str,
    node: &'static ConditionDescriptor,
    negated: bool,
    graph: &mut BTreeMap<&'static str, Vec<AvailabilityEdge>>,
) {
    let targets = availability_targets(node.predicate);

    for target in targets {
        graph
            .get_mut(owner)
            .expect("validated owner")
            .push(AvailabilityEdge {
                from: owner,
                to: target_component(target),
                condition_id: node.id,
                predicate: node.predicate.kind(),
                negated,
            });
    }

    match node.predicate {
        ConditionPredicate::All(children) | ConditionPredicate::Any(children) => {
            for child in children {
                collect_edges(owner, child, negated, graph);
            }
        }
        ConditionPredicate::Not(child) => collect_edges(owner, child, !negated, graph),
        _ => {}
    }
}

fn collect_dependencies(
    node: &'static ConditionDescriptor,
    dependencies: &mut BTreeSet<ConditionDependency>,
) {
    match node.predicate {
        ConditionPredicate::ConfigBool(fact) | ConditionPredicate::ConfigEquals { fact, .. } => {
            dependencies.insert(ConditionDependency::Config(fact));
        }
        ConditionPredicate::ComponentEligible(component) => {
            dependencies.insert(ConditionDependency::Component(component));
        }
        ConditionPredicate::ProviderMappingEligible(provider) => {
            dependencies.insert(ConditionDependency::Provider(provider));
        }
        ConditionPredicate::ConfigCallback(callback) => {
            dependencies.extend(
                callback
                    .inputs
                    .iter()
                    .copied()
                    .map(ConditionDependency::Config),
            );
        }
        ConditionPredicate::AvailabilityCallback(callback) => {
            dependencies.extend(callback.inputs.iter().copied().map(|target| match target {
                AvailabilityTarget::Component(component) => {
                    ConditionDependency::Component(component)
                }
                AvailabilityTarget::ProviderMapping(provider) => {
                    ConditionDependency::Provider(provider)
                }
            }));
        }
        ConditionPredicate::All(children) | ConditionPredicate::Any(children) => {
            for child in children {
                collect_dependencies(child, dependencies);
            }
        }
        ConditionPredicate::Not(child) => collect_dependencies(child, dependencies),
    }
}

fn availability_targets(predicate: ConditionPredicate) -> Vec<AvailabilityTarget> {
    match predicate {
        ConditionPredicate::ComponentEligible(component) => {
            vec![AvailabilityTarget::Component(component)]
        }
        ConditionPredicate::ProviderMappingEligible(provider) => {
            vec![AvailabilityTarget::ProviderMapping(provider)]
        }
        ConditionPredicate::AvailabilityCallback(callback) => callback.inputs.to_vec(),
        _ => Vec::new(),
    }
}

fn target_component(target: AvailabilityTarget) -> &'static str {
    match target {
        AvailabilityTarget::Component(component) => component,
        AvailabilityTarget::ProviderMapping(provider) => provider.component,
    }
}

fn find_cycle(
    component: &'static str,
    graph: &BTreeMap<&'static str, Vec<AvailabilityEdge>>,
    visiting: &mut Vec<AvailabilityEdge>,
    active: &mut BTreeSet<&'static str>,
    completed: &mut BTreeSet<&'static str>,
) -> Option<Vec<AvailabilityEdge>> {
    if completed.contains(component) {
        return None;
    }

    active.insert(component);

    for edge in &graph[component] {
        if active.contains(edge.to) {
            let position = visiting
                .iter()
                .position(|candidate| candidate.from == edge.to)
                .unwrap_or(visiting.len());
            let mut cycle = visiting[position..].to_vec();
            cycle.push(*edge);

            return Some(cycle);
        }

        visiting.push(*edge);

        if let Some(cycle) = find_cycle(edge.to, graph, visiting, active, completed) {
            return Some(cycle);
        }

        visiting.pop();
    }

    active.remove(component);
    completed.insert(component);

    None
}
