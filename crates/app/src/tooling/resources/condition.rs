//! Projection of the conditional possibility graph next to the prepared effective graph.
//!
//! Every catalog candidate is projected, eligible or not, and labelled with whether the
//! effective graph includes it. Each condition node becomes a resource that gates its parent
//! node or component and depends on the inputs it reads. Outcomes come from the prepared
//! evaluation: a tooling preparation decides only structured predicates and pure callbacks,
//! so a trusted callback shows its declared inputs and source text as not evaluated.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use upwell_core::{
    ConditionDescriptor, ConditionInputId, ConditionPredicate, ConditionPredicateKind,
    ConditionPurity, ConditionScalarKind, ConfigFactId,
};
use upwell_di::{ComponentDescriptor, ConditionEvaluation, ConditionOutcome, ProviderDescriptor};
use upwell_tooling_schema::{RelationshipKind, ResourceKind};

use super::super::snapshot::CandidateSnapshot;
use super::super::{
    Projection, component_id, config_binding_id, descriptor_location, provider_id, scope_id,
    type_location, with_source,
};
use super::dependency::dependency_labels;
use crate::{AppRegistry, ExecutionMode, ProtocolDefinition};

/// The outcome of each `(component, condition)` node, as the evaluation decided it.
type NodeOutcomes = HashMap<(&'static str, &'static str), ConditionOutcome>;

impl<D: ProtocolDefinition> Projection<'_, D> {
    pub(in crate::tooling) fn project_conditions(&mut self) {
        let state = self.app.condition_state();
        let catalog = Arc::clone(state.catalog());
        let evaluation = state.evaluation().evaluation().clone();
        let outcomes = evaluation
            .decisions()
            .iter()
            .map(|decision| {
                (
                    (decision.component_id, decision.condition_id),
                    decision.outcome,
                )
            })
            .collect::<NodeOutcomes>();
        let projected = self
            .document
            .resources
            .iter()
            .map(|resource| resource.id.clone())
            .collect::<BTreeSet<_>>();

        self.label_condition_evaluation(&evaluation);

        let candidates = self.app.tooling_snapshot().candidates().to_vec();

        for candidate in &candidates {
            let component = &candidate.descriptor;
            let id = component_id(component.id);

            if !projected.contains(&id) {
                self.project_candidate(candidate);
            }

            self.label_candidate(&id, candidate, evaluation.component_outcome(component.id));

            if let Some(root) = component.condition {
                self.project_condition_node(component.id, root, &id, root.id, &outcomes);
            }
        }

        self.project_provider_candidates(&catalog, &projected);
    }

    fn label_condition_evaluation(&mut self, evaluation: &ConditionEvaluation) {
        let mode = match self.app.execution_mode() {
            ExecutionMode::Tooling => "tooling",
            _ => "runtime",
        };
        let graph = if evaluation.is_complete() {
            "complete"
        } else {
            "partial"
        };

        self.label(
            "application",
            [
                ("condition-evaluation", mode.to_string()),
                ("effective-graph", graph.to_string()),
            ],
        );
    }

    /// A catalog component the effective graph leaves out: its declared dependencies are
    /// possible edges to the requested types, since no selection ran for it.
    fn project_candidate(&mut self, candidate: &CandidateSnapshot) {
        let component = &candidate.descriptor;
        let id = component_id(component.id);
        let ty = self.type_resource(&component.ty);
        let scope = scope_id(component.scope.id().as_str());

        self.resource(
            &id,
            ResourceKind::Component,
            component.name,
            with_source(None, type_location(&component.ty)),
            BTreeMap::from([
                (String::from("component-id"), component.id.to_string()),
                (
                    String::from("rust-type"),
                    (component.ty.type_name)().to_string(),
                ),
                (
                    String::from("scope-id"),
                    component.scope.id().as_str().to_string(),
                ),
                (
                    String::from("construction"),
                    String::from("inactive-candidate"),
                ),
            ]),
        );
        self.relationship(RelationshipKind::Provides, &id, &ty, []);
        self.relationship(
            RelationshipKind::DependsOn,
            &id,
            &scope,
            [(String::from("role"), String::from("scope"))],
        );

        for dependency in &candidate.dependencies {
            let target = self.type_resource(&dependency.ty);
            let mut labels = dependency_labels(dependency);

            labels.insert(String::from("possible"), String::from("true"));
            self.relationship_map(RelationshipKind::DependsOn, &id, &target, labels);
        }
    }

    fn label_candidate(
        &mut self,
        id: &str,
        candidate: &CandidateSnapshot,
        outcome: Option<ConditionOutcome>,
    ) {
        let state = match (candidate.descriptor.condition, outcome) {
            (None, _) => "unconditional",
            (Some(_), Some(outcome)) => outcome.as_str(),
            (Some(_), None) => ConditionOutcome::NotEvaluated.as_str(),
        };
        let effective = state == "unconditional" || outcome == Some(ConditionOutcome::Holds);

        self.label(
            id,
            [
                ("effective", effective.to_string()),
                ("condition-state", state.to_string()),
                (
                    "transition-capabilities",
                    candidate.transition_capabilities.join(","),
                ),
            ],
        );
    }

    /// Projects `node` at `path` under `parent`, then its children and its inputs.
    fn project_condition_node(
        &mut self,
        owner: &'static str,
        node: &'static ConditionDescriptor,
        parent: &str,
        path: &str,
        outcomes: &NodeOutcomes,
    ) {
        let id = format!("condition:{owner}:{path}");
        let outcome = outcomes
            .get(&(owner, node.id))
            .copied()
            .unwrap_or(ConditionOutcome::NotEvaluated);
        let mut labels = BTreeMap::from([
            (String::from("component-id"), owner.to_string()),
            (String::from("condition-id"), node.id.to_string()),
            (
                String::from("predicate"),
                predicate_name(node.predicate.kind()).to_string(),
            ),
            (String::from("outcome"), outcome.as_str().to_string()),
        ]);

        predicate_labels(node.predicate, &mut labels);
        self.resource(
            &id,
            ResourceKind::Condition,
            node.id,
            with_source(None, descriptor_location(node.source)),
            labels,
        );
        self.relationship(RelationshipKind::Gates, &id, parent, []);

        for input in predicate_inputs(node.predicate) {
            self.project_condition_input(&id, input);
        }

        for child in children(node.predicate) {
            self.project_condition_node(
                owner,
                child,
                &id,
                &format!("{path}/{}", child.id),
                outcomes,
            );
        }
    }

    /// Links a node to the projected resource an input reads. Facts and raw paths have no
    /// resource; they are listed on the node instead.
    fn project_condition_input(&mut self, node: &str, input: ConditionInputId) {
        let (target, role) = match input {
            ConditionInputId::Binding(binding) => (
                config_binding_id(binding.type_name(), binding.path),
                "config-input",
            ),
            ConditionInputId::Component(component) => {
                (component_id(component), "eligibility-input")
            }
            ConditionInputId::ProviderMapping(mapping) => (
                component_id(mapping.component),
                "provider-eligibility-input",
            ),
            ConditionInputId::Fact(_) | ConditionInputId::Raw(_) => return,
        };

        if !self
            .document
            .resources
            .iter()
            .any(|resource| resource.id == target)
        {
            return;
        }

        self.relationship(
            RelationshipKind::DependsOn,
            node,
            &target,
            [(String::from("role"), role.to_string())],
        );
    }

    /// Projects the catalog's provider mappings the effective graph leaves out, and labels
    /// every mapping with its eligibility and fallback role.
    fn project_provider_candidates(&mut self, catalog: &AppRegistry, projected: &BTreeSet<String>) {
        let components = catalog
            .components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect::<HashMap<_, _>>();

        for provider in &catalog.providers {
            let id = provider_id(provider);
            let component = components.get(&provider.concrete_ty.type_id);

            if !projected.contains(&id) {
                self.project_provider(provider);

                if let Some(component) = component {
                    self.relationship(
                        RelationshipKind::Provides,
                        &component_id(component.id),
                        &id,
                        [],
                    );
                }
            }

            self.label(
                &id,
                [
                    ("effective", projected.contains(&id).to_string()),
                    (
                        "selection-role",
                        selection_role(provider, &components, catalog),
                    ),
                ],
            );
        }
    }

    /// Adds `labels` to the projected resource `id`.
    fn label<const N: usize>(&mut self, id: &str, labels: [(&str, String); N]) {
        let resource = self
            .document
            .resources
            .iter_mut()
            .find(|resource| resource.id == id)
            .expect("labelled resources are projected");

        for (name, value) in labels {
            resource.labels.insert(name.to_string(), value);
        }
    }
}

/// Whether a provider mapping is the unconditional fallback among conditional siblings for
/// its trait, one of those conditional candidates, or a plain provider.
fn selection_role(
    provider: &ProviderDescriptor,
    components: &HashMap<std::any::TypeId, ComponentDescriptor>,
    catalog: &AppRegistry,
) -> String {
    let conditional = |candidate: &ProviderDescriptor| {
        components
            .get(&candidate.concrete_ty.type_id)
            .is_some_and(|component| component.condition.is_some())
    };
    let siblings = catalog
        .providers
        .iter()
        .filter(|candidate| candidate.trait_ty.type_id == provider.trait_ty.type_id)
        .collect::<Vec<_>>();
    let role = if conditional(provider) {
        "conditional"
    } else if siblings.iter().any(|sibling| conditional(sibling)) {
        "fallback"
    } else {
        "unconditional"
    };

    role.to_string()
}

/// Redacted predicate metadata: kinds, inputs and source text, never compared values.
fn predicate_labels(predicate: ConditionPredicate, labels: &mut BTreeMap<String, String>) {
    match predicate {
        ConditionPredicate::ConfigBool(fact) => {
            labels.insert(String::from("fact"), fact_name(fact));
        }

        ConditionPredicate::ConfigEquals { fact, expected } => {
            labels.insert(String::from("fact"), fact_name(fact));
            labels.insert(
                String::from("expected-kind"),
                scalar_kind_name(expected.kind()).to_string(),
            );
        }

        ConditionPredicate::Callback(callback) => {
            let purity = match callback.purity {
                ConditionPurity::Pure => "pure",
                ConditionPurity::Trusted => "trusted",
            };
            let raw = callback
                .inputs
                .iter()
                .filter_map(|input| match input {
                    ConditionInputId::Raw(path) => Some(*path),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let facts = callback
                .inputs
                .iter()
                .filter_map(|input| match input {
                    ConditionInputId::Fact(fact) => Some(fact_name(*fact)),
                    _ => None,
                })
                .collect::<Vec<_>>();

            labels.insert(String::from("callback-kind"), callback.kind.to_string());
            labels.insert(String::from("purity"), purity.to_string());

            if let Some(expression) = callback.expression {
                labels.insert(String::from("expression"), expression.text.to_string());
            }

            if !raw.is_empty() {
                labels.insert(String::from("raw-inputs"), raw.join(","));
            }

            if !facts.is_empty() {
                labels.insert(String::from("fact-inputs"), facts.join(","));
            }
        }

        ConditionPredicate::ComponentEligible(_)
        | ConditionPredicate::ProviderMappingEligible(_)
        | ConditionPredicate::All(_)
        | ConditionPredicate::Any(_)
        | ConditionPredicate::Not(_) => {}
    }
}

/// The inputs a node reads itself, excluding those of its children.
fn predicate_inputs(predicate: ConditionPredicate) -> Vec<ConditionInputId> {
    match predicate {
        ConditionPredicate::ComponentEligible(component) => {
            vec![ConditionInputId::Component(component)]
        }
        ConditionPredicate::ProviderMappingEligible(mapping) => {
            vec![ConditionInputId::ProviderMapping(mapping)]
        }
        ConditionPredicate::Callback(callback) => callback.inputs.to_vec(),
        ConditionPredicate::ConfigBool(_)
        | ConditionPredicate::ConfigEquals { .. }
        | ConditionPredicate::All(_)
        | ConditionPredicate::Any(_)
        | ConditionPredicate::Not(_) => Vec::new(),
    }
}

fn children(predicate: ConditionPredicate) -> &'static [ConditionDescriptor] {
    match predicate {
        ConditionPredicate::All(children) | ConditionPredicate::Any(children) => children,
        ConditionPredicate::Not(child) => std::slice::from_ref(child),
        _ => &[],
    }
}

fn fact_name(fact: ConfigFactId) -> String {
    format!(
        "{}@{}#{}",
        fact.config_type, fact.binding_path, fact.property_path
    )
}

fn predicate_name(kind: ConditionPredicateKind) -> &'static str {
    match kind {
        ConditionPredicateKind::ConfigBool => "config-bool",
        ConditionPredicateKind::ConfigEquals => "config-equals",
        ConditionPredicateKind::ComponentEligible => "component-eligible",
        ConditionPredicateKind::ProviderMappingEligible => "provider-mapping-eligible",
        ConditionPredicateKind::Callback => "callback",
        ConditionPredicateKind::All => "all",
        ConditionPredicateKind::Any => "any",
        ConditionPredicateKind::Not => "not",
    }
}

fn scalar_kind_name(kind: ConditionScalarKind) -> &'static str {
    match kind {
        ConditionScalarKind::Bool => "bool",
        ConditionScalarKind::Integer => "integer",
        ConditionScalarKind::String => "string",
        ConditionScalarKind::EnumToken => "enum-token",
    }
}
