use std::collections::BTreeSet;

use upwell_tooling_schema::{RelationshipKind, Resource, ResourceKind};

use super::index::GraphIndex;
use super::model::{ConditionStep, ProviderSelection, ResolvedConsumer};

/// The provider selection that decides `resource`: every mapping of each trait the
/// resource is, implements, or provides, with the condition path gating its component.
///
/// It reads the probe's selection labels and gating relationships; nothing is
/// re-evaluated.
pub(super) fn provider_selection(
    index: &GraphIndex,
    resource: &Resource,
) -> Vec<ProviderSelection> {
    let traits = match resource.kind {
        ResourceKind::Type => BTreeSet::from([resource.id.as_str()]),
        ResourceKind::Provider => provided_traits(index, [resource.id.as_str()]),
        ResourceKind::Component => provided_traits(
            index,
            index.neighbors(&resource.id, RelationshipKind::Provides, true),
        ),
        _ => BTreeSet::new(),
    };

    traits
        .into_iter()
        .flat_map(|ty| index.neighbors(ty, RelationshipKind::Provides, false))
        .filter(|provider| index.resource(provider).kind == ResourceKind::Provider)
        .map(|provider| selection(index, provider))
        .collect()
}

fn provided_traits<'a>(
    index: &'a GraphIndex,
    providers: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<&'a str> {
    providers
        .into_iter()
        .filter(|provider| index.resource(provider).kind == ResourceKind::Provider)
        .flat_map(|provider| index.neighbors(provider, RelationshipKind::Provides, true))
        .collect()
}

fn selection(index: &GraphIndex, provider: &str) -> ProviderSelection {
    let resource = index.resource(provider);
    let component = index
        .neighbors(provider, RelationshipKind::Provides, false)
        .into_iter()
        .find(|component| index.resource(component).kind == ResourceKind::Component);
    let mut conditions = Vec::new();

    if let Some(component) = component {
        collect_conditions(index, component, 0, &mut conditions);
    }

    ProviderSelection {
        provider: provider.to_string(),
        component: component.map(str::to_string),
        effective: label(resource, "effective") == Some("true"),
        role: label(resource, "selection-role").map(str::to_string),
        condition_state: component
            .and_then(|component| label(index.resource(component), "condition-state"))
            .map(str::to_string),
        conditions,
        resolved_by: resolved_by(index, provider),
    }
}

fn resolved_by(index: &GraphIndex, provider: &str) -> Vec<ResolvedConsumer> {
    index
        .relationships_for(provider, false)
        .into_iter()
        .filter(|edge| edge.kind == RelationshipKind::DependsOn)
        .filter(|edge| edge.labels.get("role").map(String::as_str) == Some("resolved-provider"))
        .map(|edge| ResolvedConsumer {
            reason: edge.labels.get("selection-reason").cloned(),
            consumer: edge.from,
        })
        .collect()
}

/// Collects the conditions gating `gated`, depth-first, each followed by its children.
fn collect_conditions(
    index: &GraphIndex,
    gated: &str,
    depth: usize,
    conditions: &mut Vec<ConditionStep>,
) {
    for condition in index.neighbors(gated, RelationshipKind::Gates, false) {
        let resource = index.resource(condition);

        conditions.push(ConditionStep {
            id: condition.to_string(),
            depth,
            predicate: label(resource, "predicate").map(str::to_string),
            outcome: label(resource, "outcome").map(str::to_string),
            detail: ["expression", "fact"]
                .into_iter()
                .find_map(|name| label(resource, name))
                .map(str::to_string),
        });
        collect_conditions(index, condition, depth + 1, conditions);
    }
}

fn label<'a>(resource: &'a Resource, name: &str) -> Option<&'a str> {
    resource.labels.get(name).map(String::as_str)
}
