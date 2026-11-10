use upwell_tooling_schema::{Relationship, RelationshipKind, ResourceKind};

use super::index::GraphIndex;
use super::model::{GraphDirection, GraphRelationFamily};

pub(super) fn relation_matches(
    index: &GraphIndex,
    edge: &Relationship,
    requested: GraphRelationFamily,
) -> bool {
    requested == GraphRelationFamily::All || relation_family(index, edge) == Some(requested)
}

fn relation_family(index: &GraphIndex, edge: &Relationship) -> Option<GraphRelationFamily> {
    let from = &index.resource(&edge.from).kind;
    let to = &index.resource(&edge.to).kind;
    let composition = is_composition_resource(from) || is_composition_resource(to);

    match edge.kind {
        RelationshipKind::DependsOn
            if edge.labels.get("role").is_some_and(|role| role == "scope") =>
        {
            Some(GraphRelationFamily::Scopes)
        }
        RelationshipKind::DependsOn if composition => Some(GraphRelationFamily::Composition),
        RelationshipKind::DependsOn | RelationshipKind::Binds => {
            Some(GraphRelationFamily::Dependencies)
        }
        RelationshipKind::Provides if composition => Some(GraphRelationFamily::Composition),
        RelationshipKind::Provides | RelationshipKind::Gates => {
            Some(GraphRelationFamily::Dependencies)
        }
        RelationshipKind::OpensScope | RelationshipKind::Enters => {
            Some(GraphRelationFamily::Scopes)
        }
        RelationshipKind::Hooks => Some(GraphRelationFamily::Lifecycle),
        RelationshipKind::Contains
        | RelationshipKind::Contributes
        | RelationshipKind::Validates => Some(GraphRelationFamily::Ownership),
        RelationshipKind::Replaces | RelationshipKind::Suppresses | RelationshipKind::Conflicts => {
            Some(GraphRelationFamily::Composition)
        }
        RelationshipKind::OrdersBefore | RelationshipKind::OrdersAfter if composition => {
            Some(GraphRelationFamily::Composition)
        }
        RelationshipKind::OrdersBefore | RelationshipKind::OrdersAfter
            if *from == ResourceKind::Lifecycle || *to == ResourceKind::Lifecycle =>
        {
            Some(GraphRelationFamily::Lifecycle)
        }
        RelationshipKind::OrdersBefore | RelationshipKind::OrdersAfter => {
            Some(GraphRelationFamily::Dependencies)
        }
        _ => None,
    }
}

pub(super) fn traversal_neighbors<'a>(
    edge: &'a Relationship,
    current: &str,
    direction: GraphDirection,
) -> Vec<&'a str> {
    if edge.kind == RelationshipKind::Conflicts {
        return opposite_endpoint(edge, current).into_iter().collect();
    }

    let (effect, explanation) = semantic_orientation(edge);

    match direction {
        GraphDirection::Upstream if current == effect => vec![explanation],
        GraphDirection::Downstream if current == explanation => vec![effect],
        GraphDirection::Both if current == effect => vec![explanation],
        GraphDirection::Both if current == explanation => vec![effect],
        _ => Vec::new(),
    }
}

fn semantic_orientation(edge: &Relationship) -> (&str, &str) {
    match edge.kind {
        RelationshipKind::Provides
        | RelationshipKind::Binds
        | RelationshipKind::OpensScope
        | RelationshipKind::Contains
        | RelationshipKind::Contributes
        | RelationshipKind::Replaces
        | RelationshipKind::Suppresses
        | RelationshipKind::OrdersBefore
        | RelationshipKind::Validates => (&edge.to, &edge.from),
        RelationshipKind::DependsOn | RelationshipKind::OrdersAfter => (&edge.from, &edge.to),
        RelationshipKind::Hooks | RelationshipKind::Gates => (&edge.to, &edge.from),
        _ => (&edge.from, &edge.to),
    }
}

fn opposite_endpoint<'a>(edge: &'a Relationship, current: &str) -> Option<&'a str> {
    if current == edge.from {
        Some(&edge.to)
    } else if current == edge.to {
        Some(&edge.from)
    } else {
        None
    }
}

pub(super) fn is_contributor(kind: &ResourceKind) -> bool {
    matches!(
        kind,
        ResourceKind::Application
            | ResourceKind::Protocol
            | ResourceKind::Plugin
            | ResourceKind::Contributor
    )
}

fn is_composition_resource(kind: &ResourceKind) -> bool {
    matches!(kind, ResourceKind::Plugin | ResourceKind::PluginSlot)
}
