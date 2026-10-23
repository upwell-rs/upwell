//! Labels and inputs of condition predicates, as tooling projects them.

use std::collections::{BTreeMap, HashMap};

use upwell_core::{
    ConditionDescriptor, ConditionInputId, ConditionPredicate, ConditionPredicateKind,
    ConditionPurity, ConditionScalarKind, ConfigFactId,
};
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

use crate::AppRegistry;

/// Whether a provider mapping is the unconditional fallback among conditional siblings for
/// its trait, one of those conditional candidates, or a plain provider.
pub(super) fn selection_role(
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
pub(super) fn predicate_labels(
    predicate: ConditionPredicate,
    labels: &mut BTreeMap<String, String>,
) {
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
pub(super) fn predicate_inputs(predicate: ConditionPredicate) -> Vec<ConditionInputId> {
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

pub(super) fn children(predicate: ConditionPredicate) -> &'static [ConditionDescriptor] {
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

pub(super) fn predicate_name(kind: ConditionPredicateKind) -> &'static str {
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
