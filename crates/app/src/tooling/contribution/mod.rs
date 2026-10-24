use std::collections::{BTreeMap, BTreeSet};

use upwell_core::ItemSource;
use upwell_tooling_schema::{
    Facet, Provenance, Relationship, RelationshipKind, Resource, ResourceDisplay, ResourceKind,
    SourceLocation,
};

pub use error::ToolingContributionError;

use pending::{
    PendingAttribute, PendingEndpoint, PendingFacet, PendingRelationship, PendingResource, qualify,
    validate_endpoint, validate_local_id, validate_owner, validate_relationship_endpoints,
};

mod error;
mod pending;

/// One endpoint in an owner-scoped tooling relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolingEndpoint<'a> {
    /// The protocol or plugin that owns this collector.
    Owner,
    /// A resource declared in this collector, addressed by its owner-local identity.
    Resource(&'a str),
}

/// Relationship semantics available to protocol and plugin tooling extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolingRelationshipKind {
    /// The source contains the target.
    Contains,
    /// The source depends on the target.
    DependsOn,
    /// The source must precede the target.
    OrdersBefore,
    /// The source must follow the target.
    OrdersAfter,
}

impl From<ToolingRelationshipKind> for RelationshipKind {
    fn from(kind: ToolingRelationshipKind) -> Self {
        match kind {
            ToolingRelationshipKind::Contains => Self::Contains,
            ToolingRelationshipKind::DependsOn => Self::DependsOn,
            ToolingRelationshipKind::OrdersBefore => Self::OrdersBefore,
            ToolingRelationshipKind::OrdersAfter => Self::OrdersAfter,
        }
    }
}

/// Generic, protocol-neutral metadata contributed by one prepared protocol or plugin.
///
/// Resource and facet identities supplied to this collector are owner-local. The framework
/// qualifies them with the protocol or plugin identity when preparation is projected, preventing
/// an extension from claiming framework or another contributor's resources.
#[derive(Debug)]
pub struct ToolingContributions {
    owner: String,
    resources: Vec<PendingResource>,
    relationships: Vec<PendingRelationship>,
    facets: Vec<PendingFacet>,
    displays: Vec<PendingAttribute<ResourceDisplay>>,
    sources: Vec<PendingAttribute<SourceLocation>>,
    owner_display: Option<ResourceDisplay>,
}

impl ToolingContributions {
    pub(crate) fn new(owner: String) -> Self {
        Self {
            owner,
            resources: Vec::new(),
            relationships: Vec::new(),
            facets: Vec::new(),
            displays: Vec::new(),
            sources: Vec::new(),
            owner_display: None,
        }
    }

    /// Declares presentation for this protocol or plugin owner.
    pub fn display(&mut self, display: ResourceDisplay) {
        self.owner_display = Some(display);
    }

    /// Declares presentation for one owner-local resource.
    pub fn resource_display(&mut self, resource: impl Into<String>, display: ResourceDisplay) {
        self.displays.push(PendingAttribute {
            resource: resource.into(),
            value: display,
        });
    }

    /// Declares the Rust item one owner-local resource was declared by, so tooling can
    /// navigate to it. Unknown positions (a zero line) are ignored.
    pub fn resource_source(&mut self, resource: impl Into<String>, source: ItemSource) {
        if let Some(location) = crate::tooling::item_location(&source) {
            self.sources.push(PendingAttribute {
                resource: resource.into(),
                value: location,
            });
        }
    }

    /// Declares an owner-local contribution resource without scalar labels.
    pub fn resource(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.resource_with_labels(id, name, BTreeMap::new());
    }

    /// Declares an owner-local contribution resource with deterministic scalar labels.
    pub fn resource_with_labels(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        labels: BTreeMap<String, String>,
    ) {
        self.resources.push(PendingResource {
            id: id.into(),
            name: name.into(),
            labels,
        });
    }

    /// Declares a relationship whose endpoints are this owner or owner-local resources.
    pub fn relationship(
        &mut self,
        kind: ToolingRelationshipKind,
        from: ToolingEndpoint<'_>,
        to: ToolingEndpoint<'_>,
    ) {
        self.relationship_with_labels(kind, from, to, BTreeMap::new());
    }

    /// Declares a relationship with deterministic scalar labels.
    pub fn relationship_with_labels(
        &mut self,
        kind: ToolingRelationshipKind,
        from: ToolingEndpoint<'_>,
        to: ToolingEndpoint<'_>,
        labels: BTreeMap<String, String>,
    ) {
        self.relationships.push(PendingRelationship {
            kind: kind.into(),
            from: PendingEndpoint::from(from),
            to: PendingEndpoint::from(to),
            labels,
        });
    }

    /// Attaches an owner-local opaque facet to the protocol or plugin itself.
    pub fn facet(
        &mut self,
        id: impl Into<String>,
        schema_version: u16,
        value: upwell_tooling_schema::JsonValue,
    ) {
        self.facets.push(PendingFacet {
            resource: None,
            id: id.into(),
            facet: Facet {
                schema_version,
                value,
            },
        });
    }

    /// Attaches an owner-local opaque facet to a resource in this collector.
    pub fn resource_facet(
        &mut self,
        resource: impl Into<String>,
        id: impl Into<String>,
        schema_version: u16,
        value: upwell_tooling_schema::JsonValue,
    ) {
        self.facets.push(PendingFacet {
            resource: Some(resource.into()),
            id: id.into(),
            facet: Facet {
                schema_version,
                value,
            },
        });
    }

    pub(crate) fn finish(self) -> Result<ToolingContributionSet, ToolingContributionError> {
        let mut identities = BTreeSet::new();
        let mut resources = Vec::with_capacity(self.resources.len());

        validate_owner(&self.owner)?;

        if self.owner.starts_with("protocol:") && self.owner_display.is_none() {
            return Err(ToolingContributionError::MissingProtocolDisplay {
                id: self.owner.clone(),
            });
        }

        for resource in self.resources {
            validate_local_id(&resource.id)?;

            if resource.name.trim().is_empty() {
                return Err(ToolingContributionError::EmptyResourceName { id: resource.id });
            }

            if !identities.insert(resource.id.clone()) {
                return Err(ToolingContributionError::DuplicateResource { id: resource.id });
            }

            resources.push(Resource {
                id: qualify(&self.owner, &resource.id),
                kind: ResourceKind::Contribution,
                name: resource.name,
                display: None,
                provenance: Some(Provenance {
                    owner: Some(self.owner.clone()),
                    origin: Some(String::from("tooling-contribution")),
                    ..Provenance::default()
                }),
                labels: resource.labels,
                facets: BTreeMap::new(),
            });
        }

        attach(
            &self.owner,
            &identities,
            &mut resources,
            self.displays,
            |id| ToolingContributionError::DuplicateDisplay { id },
            |resource, display| resource.display = Some(display),
        )?;
        attach(
            &self.owner,
            &identities,
            &mut resources,
            self.sources,
            |id| ToolingContributionError::DuplicateSource { id },
            |resource, source| {
                resource
                    .provenance
                    .get_or_insert_with(Provenance::default)
                    .source = Some(source);
            },
        )?;

        let mut relationship_identities = BTreeSet::new();
        let mut relationships = Vec::with_capacity(self.relationships.len());

        for relationship in self.relationships {
            validate_endpoint(&relationship.from, &identities)?;
            validate_endpoint(&relationship.to, &identities)?;
            validate_relationship_endpoints(
                &relationship.kind,
                &relationship.from,
                &relationship.to,
            )?;

            let relationship = Relationship {
                kind: relationship.kind,
                from: relationship.from.qualify(&self.owner),
                to: relationship.to.qualify(&self.owner),
                labels: relationship.labels,
            };
            let identity = (
                relationship.kind.clone(),
                relationship.from.clone(),
                relationship.to.clone(),
                relationship.labels.clone(),
            );

            if !relationship_identities.insert(identity) {
                return Err(ToolingContributionError::DuplicateRelationship {
                    from: relationship.from,
                    to: relationship.to,
                });
            }

            relationships.push(relationship);
        }

        let mut facet_identities = BTreeSet::new();
        let mut owner_facets = BTreeMap::new();

        for facet in self.facets {
            validate_local_id(&facet.id)?;

            if facet.facet.schema_version == 0 {
                return Err(ToolingContributionError::InvalidFacetVersion { id: facet.id });
            }

            let target = match facet.resource {
                Some(resource) => {
                    validate_local_id(&resource)?;

                    if !identities.contains(&resource) {
                        return Err(ToolingContributionError::UnknownEndpoint { id: resource });
                    }

                    qualify(&self.owner, &resource)
                }
                None => self.owner.clone(),
            };
            let id = qualify(&target, &facet.id);

            if !facet_identities.insert(id.clone()) {
                return Err(ToolingContributionError::DuplicateFacet { id });
            }

            if target == self.owner {
                owner_facets.insert(id, facet.facet);
            } else {
                let resource = resources
                    .iter_mut()
                    .find(|resource| resource.id == target)
                    .expect("validated contributed resource exists");

                resource.facets.insert(id, facet.facet);
            }
        }

        Ok(ToolingContributionSet {
            owner: self.owner,
            resources,
            relationships,
            owner_facets,
            owner_display: self.owner_display,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Validated owner-qualified resources, relationships, and facets ready for projection.
pub(crate) struct ToolingContributionSet {
    pub(crate) owner: String,
    pub(crate) resources: Vec<Resource>,
    pub(crate) relationships: Vec<Relationship>,
    pub(crate) owner_facets: BTreeMap<String, Facet>,
    pub(crate) owner_display: Option<ResourceDisplay>,
}

/// Applies owner-local resource attributes to their validated resources, rejecting unknown
/// targets and a second attribute for one resource.
fn attach<T>(
    owner: &str,
    identities: &BTreeSet<String>,
    resources: &mut [Resource],
    attributes: Vec<PendingAttribute<T>>,
    duplicate: fn(String) -> ToolingContributionError,
    apply: fn(&mut Resource, T),
) -> Result<(), ToolingContributionError> {
    let mut attached = BTreeSet::new();

    for attribute in attributes {
        validate_local_id(&attribute.resource)?;

        if !identities.contains(&attribute.resource) {
            return Err(ToolingContributionError::UnknownEndpoint {
                id: attribute.resource,
            });
        }

        if !attached.insert(attribute.resource.clone()) {
            return Err(duplicate(attribute.resource));
        }

        let qualified = qualify(owner, &attribute.resource);
        let resource = resources
            .iter_mut()
            .find(|resource| resource.id == qualified)
            .expect("validated contributed resource exists");

        apply(resource, attribute.value);
    }

    Ok(())
}

#[cfg(test)]
mod tests;
