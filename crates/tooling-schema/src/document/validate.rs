//! Structural validation of tooling document resources and relationships.

use crate::*;

pub(crate) fn validate_tooling_document_identity(
    identity: &DocumentIdentity,
) -> Result<(), ValidationError> {
    if is_blank(&identity.application) {
        return Err(ValidationError::MissingApplicationIdentity);
    }

    if identity
        .package
        .as_ref()
        .is_some_and(|package| validate_package_identity(package).is_err())
        || identity
            .binary
            .as_ref()
            .is_some_and(|binary| is_blank(&binary.name))
        || identity
            .source
            .as_ref()
            .is_some_and(|source| validate_source_location(source).is_err())
    {
        return Err(ValidationError::InvalidDocumentIdentity);
    }

    Ok(())
}

pub(crate) fn validate_facets(facets: &BTreeMap<String, Facet>) -> Result<(), ValidationError> {
    for (id, facet) in facets {
        if !is_namespaced(id) {
            return Err(ValidationError::InvalidFacetId { id: id.clone() });
        }

        if facet.schema_version == 0 {
            return Err(ValidationError::InvalidFacetVersion { id: id.clone() });
        }
    }

    Ok(())
}

pub(crate) fn validate_provenance(
    resource: &Resource,
    resources: &BTreeMap<&str, &Resource>,
) -> Result<(), ValidationError> {
    let Some(provenance) = resource.provenance.as_ref() else {
        return Ok(());
    };
    if let Some(source) = provenance.source.as_ref() {
        match validate_source_location(source) {
            Ok(()) => {}
            Err(IdentityValidationError::MissingSource) => {
                return Err(ValidationError::EmptyProvenanceSource {
                    id: resource.id.clone(),
                });
            }
            Err(_) => {
                return Err(ValidationError::InvalidProvenanceSource {
                    id: resource.id.clone(),
                });
            }
        }
    }

    let Some(owner) = provenance.owner.as_deref() else {
        return Ok(());
    };
    let Some(owner_resource) = resources.get(owner) else {
        return Err(ValidationError::UnknownProvenanceOwner {
            id: resource.id.clone(),
            owner: owner.to_string(),
        });
    };
    let owner_kind = &owner_resource.kind;
    let allowed = matches!(
        owner_kind,
        ResourceKind::Application
            | ResourceKind::Protocol
            | ResourceKind::Plugin
            | ResourceKind::Contributor
    );

    if !allowed {
        return Err(ValidationError::InvalidProvenanceOwnerKind {
            id: resource.id.clone(),
            owner: owner.to_string(),
            kind: owner_kind.clone(),
        });
    }

    if owner == resource.id && !allowed_provenance_self_owner(&resource.kind) {
        return Err(ValidationError::SelfOwnedResource {
            id: resource.id.clone(),
        });
    }

    Ok(())
}

pub(crate) fn validate_resource_display(resource: &Resource) -> Result<(), ValidationError> {
    const MAX_TEXT_BYTES: usize = 16 * 1024;

    let Some(display) = &resource.display else {
        return Ok(());
    };

    if display.is_empty() {
        return Err(ValidationError::EmptyResourceDisplay {
            id: resource.id.clone(),
        });
    }

    let optional = display
        .label
        .iter()
        .chain(&display.group)
        .chain(&display.summary);

    if optional.clone().any(|value| is_blank(value))
        || display
            .details
            .iter()
            .any(|(name, value)| is_blank(name) || is_blank(value))
    {
        return Err(ValidationError::BlankResourceDisplayText {
            id: resource.id.clone(),
        });
    }

    if optional
        .chain(display.details.keys())
        .chain(display.details.values())
        .any(|value| value.len() > MAX_TEXT_BYTES)
    {
        return Err(ValidationError::ResourceDisplayTextTooLarge {
            id: resource.id.clone(),
        });
    }

    Ok(())
}

pub(crate) fn allowed_provenance_self_owner(kind: &ResourceKind) -> bool {
    matches!(
        kind,
        ResourceKind::Application
            | ResourceKind::Protocol
            | ResourceKind::Plugin
            | ResourceKind::Contributor
    )
}

pub(crate) fn relationship_resource<'a>(
    resources: &'a BTreeMap<&str, &'a Resource>,
    id: &str,
) -> Result<&'a Resource, ValidationError> {
    resources
        .get(id)
        .copied()
        .ok_or_else(|| ValidationError::UnknownRelationshipResource { id: id.to_string() })
}

pub(crate) fn validate_relationship_kinds(
    relationship: &Relationship,
    from: &Resource,
    to: &Resource,
) -> Result<(), ValidationError> {
    let valid = match relationship.kind {
        RelationshipKind::OpensScope => {
            from.kind == ResourceKind::Scope && to.kind == ResourceKind::Scope
        }
        RelationshipKind::Provides => matches!(
            (&from.kind, &to.kind),
            (
                ResourceKind::Component,
                ResourceKind::Type | ResourceKind::Provider
            ) | (ResourceKind::Provider, ResourceKind::Type)
                | (ResourceKind::Plugin, ResourceKind::PluginSlot)
        ),
        RelationshipKind::Binds => matches!(
            (&from.kind, &to.kind),
            (ResourceKind::ConfigBinding, ResourceKind::Type)
        ),
        RelationshipKind::Replaces => {
            from.kind == ResourceKind::Plugin && to.kind == ResourceKind::Plugin
        }
        RelationshipKind::Suppresses => {
            from.kind == ResourceKind::Contribution && to.kind == ResourceKind::Plugin
        }
        RelationshipKind::Hooks => matches!(
            (&from.kind, &to.kind),
            (ResourceKind::Component, ResourceKind::Hook)
                | (ResourceKind::Hook, ResourceKind::Lifecycle)
        ),
        RelationshipKind::Gates => matches!(
            (&from.kind, &to.kind),
            (
                ResourceKind::Condition,
                ResourceKind::Component | ResourceKind::Condition
            )
        ),
        RelationshipKind::Contains => valid_contains_relationship(&from.kind, &to.kind),
        RelationshipKind::Contributes => valid_contribution_relationship(&from.kind, &to.kind),
        _ => true,
    };

    if !valid {
        return Err(ValidationError::InvalidRelationshipKinds {
            kind: relationship.kind.clone(),
            from: relationship.from.clone(),
            from_kind: from.kind.clone(),
            to: relationship.to.clone(),
            to_kind: to.kind.clone(),
        });
    }

    Ok(())
}

fn valid_contains_relationship(from: &ResourceKind, to: &ResourceKind) -> bool {
    let owner = matches!(from, ResourceKind::Protocol | ResourceKind::Plugin);

    (owner || *from == ResourceKind::Contribution) && *to == ResourceKind::Contribution
}

fn valid_contribution_relationship(from: &ResourceKind, to: &ResourceKind) -> bool {
    let owner = matches!(from, ResourceKind::Plugin | ResourceKind::Contributor);
    let owned = *to == ResourceKind::Contribution;
    let projected_target = matches!(
        to,
        ResourceKind::Component | ResourceKind::Provider | ResourceKind::ConfigBinding
    );

    (owner && owned)
        || (*from == ResourceKind::Contribution
            && (*to == ResourceKind::Contribution || projected_target))
}

pub(crate) fn validate_resource_facets(resource: &Resource) -> Result<(), ValidationError> {
    validate_facets(&resource.facets)?;

    let owner_prefix = format!("{}/", resource.id);

    for id in resource.facets.keys() {
        if !id.starts_with(&owner_prefix) {
            return Err(ValidationError::InvalidFacetOwner {
                id: id.clone(),
                owner: resource.id.clone(),
            });
        }
    }

    Ok(())
}
