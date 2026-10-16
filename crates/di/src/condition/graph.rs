use std::collections::{BTreeMap, BTreeSet};

use upwell_core::{
    ConditionDescriptor, ConditionInputId, ConditionPredicate, ConditionPredicateKind,
    ConfigBindingId, ConfigFactId, ProviderMappingId,
};

use super::{ConditionCatalog, ConditionError};
use crate::{ComponentDescriptor, ComponentRegistry};

/// One fact whose change can invalidate a component's condition decision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConditionDependency {
    Config(ConfigFactId),
    Binding(ConfigBindingId),
    Raw(&'static str),
    Component(&'static str),
    Provider(ProviderMappingId),
}

impl From<ConditionInputId> for ConditionDependency {
    fn from(input: ConditionInputId) -> Self {
        match input {
            ConditionInputId::Fact(fact) => Self::Config(fact),
            ConditionInputId::Binding(binding) => Self::Binding(binding),
            ConditionInputId::Raw(path) => Self::Raw(path),
            ConditionInputId::Component(component) => Self::Component(component),
            ConditionInputId::ProviderMapping(provider) => Self::Provider(provider),
        }
    }
}

impl ComponentRegistry {
    /// The typed bindings and raw paths read by any registered condition callback, which a
    /// [`ConditionFactSnapshot`](super::ConditionFactSnapshot) must supply.
    pub fn condition_config_inputs(&self) -> BTreeSet<ConditionInputId> {
        config_inputs(&self.components)
    }
}

/// The declared binding and raw-path callback inputs of `components`' conditions.
pub(super) fn config_inputs<'a>(
    components: impl IntoIterator<Item = &'a ComponentDescriptor>,
) -> BTreeSet<ConditionInputId> {
    let mut inputs = BTreeSet::new();

    for component in components {
        if let Some(root) = component.condition {
            collect_config_inputs(root, &mut inputs);
        }
    }

    inputs
}

fn collect_config_inputs(
    node: &'static ConditionDescriptor,
    inputs: &mut BTreeSet<ConditionInputId>,
) {
    match node.predicate {
        ConditionPredicate::Callback(callback) => {
            inputs.extend(callback.inputs.iter().filter(|input| {
                matches!(
                    input,
                    ConditionInputId::Binding(_) | ConditionInputId::Raw(_)
                )
            }))
        }
        ConditionPredicate::All(children) | ConditionPredicate::Any(children) => {
            for child in children {
                collect_config_inputs(child, inputs);
            }
        }
        ConditionPredicate::Not(child) => collect_config_inputs(child, inputs),
        _ => {}
    }
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
                to: target,
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
        ConditionPredicate::Callback(callback) => {
            dependencies.extend(
                callback
                    .inputs
                    .iter()
                    .copied()
                    .map(ConditionDependency::from),
            );
        }
        ConditionPredicate::All(children) | ConditionPredicate::Any(children) => {
            for child in children {
                collect_dependencies(child, dependencies);
            }
        }
        ConditionPredicate::Not(child) => collect_dependencies(child, dependencies),
    }
}

fn availability_targets(predicate: ConditionPredicate) -> Vec<&'static str> {
    match predicate {
        ConditionPredicate::ComponentEligible(component) => vec![component],
        ConditionPredicate::ProviderMappingEligible(provider) => vec![provider.component],
        ConditionPredicate::Callback(callback) => callback
            .inputs
            .iter()
            .filter_map(|input| match input {
                ConditionInputId::Component(component) => Some(*component),
                ConditionInputId::ProviderMapping(provider) => Some(provider.component),
                ConditionInputId::Fact(_)
                | ConditionInputId::Binding(_)
                | ConditionInputId::Raw(_) => None,
            })
            .collect(),
        _ => Vec::new(),
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
