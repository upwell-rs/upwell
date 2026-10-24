use std::collections::{BTreeMap, BTreeSet};

use upwell_tooling_schema::{Facet, RelationshipKind};

use super::{ToolingContributionError, ToolingEndpoint};

#[derive(Debug)]
/// One owner-local resource awaiting validation and qualification.
pub(super) struct PendingResource {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) labels: BTreeMap<String, String>,
}

#[derive(Debug)]
/// One owner-local resource attribute (display, source) awaiting its resource.
pub(super) struct PendingAttribute<T> {
    pub(super) resource: String,
    pub(super) value: T,
}

#[derive(Debug)]
/// One owner-local relationship awaiting endpoint validation and qualification.
pub(super) struct PendingRelationship {
    pub(super) kind: RelationshipKind,
    pub(super) from: PendingEndpoint,
    pub(super) to: PendingEndpoint,
    pub(super) labels: BTreeMap<String, String>,
}

#[derive(Debug)]
/// One owner-local facet awaiting target validation and qualification.
pub(super) struct PendingFacet {
    pub(super) resource: Option<String>,
    pub(super) id: String,
    pub(super) facet: Facet,
}

#[derive(Debug)]
/// Internal owned form of a public tooling endpoint.
pub(super) enum PendingEndpoint {
    Owner,
    Resource(String),
}

impl PendingEndpoint {
    pub(super) fn qualify(&self, owner: &str) -> String {
        match self {
            Self::Owner => owner.to_string(),
            Self::Resource(id) => qualify(owner, id),
        }
    }
}

impl From<ToolingEndpoint<'_>> for PendingEndpoint {
    fn from(endpoint: ToolingEndpoint<'_>) -> Self {
        match endpoint {
            ToolingEndpoint::Owner => Self::Owner,
            ToolingEndpoint::Resource(id) => Self::Resource(id.to_string()),
        }
    }
}

pub(super) fn validate_endpoint(
    endpoint: &PendingEndpoint,
    resources: &BTreeSet<String>,
) -> Result<(), ToolingContributionError> {
    let PendingEndpoint::Resource(id) = endpoint else {
        return Ok(());
    };

    validate_local_id(id)?;

    if !resources.contains(id) {
        return Err(ToolingContributionError::UnknownEndpoint { id: id.clone() });
    }

    Ok(())
}

pub(super) fn validate_relationship_endpoints(
    kind: &RelationshipKind,
    from: &PendingEndpoint,
    to: &PendingEndpoint,
) -> Result<(), ToolingContributionError> {
    let valid = match kind {
        RelationshipKind::Contains => matches!(
            (from, to),
            (
                PendingEndpoint::Owner | PendingEndpoint::Resource(_),
                PendingEndpoint::Resource(_)
            )
        ),
        RelationshipKind::DependsOn
        | RelationshipKind::OrdersBefore
        | RelationshipKind::OrdersAfter => matches!(
            (from, to),
            (PendingEndpoint::Resource(_), PendingEndpoint::Resource(_))
        ),
        _ => false,
    };

    if !valid {
        return Err(ToolingContributionError::InvalidRelationshipEndpoints);
    }

    Ok(())
}

pub(super) fn validate_local_id(id: &str) -> Result<(), ToolingContributionError> {
    if id.trim().is_empty() {
        return Err(ToolingContributionError::EmptyId);
    }

    if id == "framework"
        || id == "application"
        || [
            "component:",
            "config-binding:",
            "contribution:",
            "hook:",
            "lifecycle:",
            "plugin:",
            "plugin-slot:",
            "protocol:",
            "provider:",
            "scope:",
            "type:",
        ]
        .iter()
        .any(|prefix| id.starts_with(prefix))
    {
        return Err(ToolingContributionError::ReservedId { id: id.to_string() });
    }

    Ok(())
}

pub(super) fn validate_owner(owner: &str) -> Result<(), ToolingContributionError> {
    let valid = ["plugin:", "protocol:"].iter().any(|prefix| {
        owner.strip_prefix(prefix).is_some_and(|id| {
            let mut parts = id.split('/');

            matches!(
                (parts.next(), parts.next()),
                (Some(namespace), Some(name)) if !namespace.is_empty() && !name.is_empty()
            )
        })
    });

    if !valid {
        return Err(ToolingContributionError::InvalidOwner {
            id: owner.to_string(),
        });
    }

    Ok(())
}

pub(super) fn qualify(owner: &str, local: &str) -> String {
    let local = local.replace('%', "%25").replace('/', "%2F");

    format!("{owner}/tooling/{local}")
}
