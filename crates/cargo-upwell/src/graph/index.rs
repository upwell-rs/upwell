use std::collections::{BTreeMap, BTreeSet};

use upwell_tooling_schema::{
    CliProvider, Relationship, RelationshipKind, Resource, ResourceKind, ToolingDocument,
};

use super::error::{GraphQueryError, GraphSelectorKind};
use super::model::{
    CliOwnershipSummary, GraphDirection, GraphQuery, GraphRelationFamily, GraphSource,
};
use super::relation::{is_contributor, relation_matches, traversal_neighbors};

pub(super) struct ResolvedGraphRoots {
    pub(super) roots: BTreeSet<String>,
    pub(super) owner_roots: BTreeSet<String>,
}

pub(super) struct GraphIndex {
    pub(super) document: ToolingDocument,
    resources: BTreeMap<String, usize>,
    names: BTreeMap<String, Vec<String>>,
    incoming: BTreeMap<String, Vec<usize>>,
    outgoing: BTreeMap<String, Vec<usize>>,
    provenance_owned: BTreeMap<String, BTreeSet<String>>,
}

impl GraphIndex {
    pub(super) fn new(document: &ToolingDocument) -> Result<Self, GraphQueryError> {
        let mut document = document.clone();

        document.canonicalize();
        document
            .validate()
            .map_err(|source| GraphQueryError::InvalidDocument {
                source: Box::new(source),
            })?;

        let mut resources = BTreeMap::new();
        let mut names = BTreeMap::<String, Vec<String>>::new();
        let mut incoming = BTreeMap::<String, Vec<usize>>::new();
        let mut outgoing = BTreeMap::<String, Vec<usize>>::new();
        let mut provenance_owned = BTreeMap::<String, BTreeSet<String>>::new();

        for (index, resource) in document.resources.iter().enumerate() {
            resources.insert(resource.id.clone(), index);
            names
                .entry(resource.name.clone())
                .or_default()
                .push(resource.id.clone());

            if let Some(owner) = resource
                .provenance
                .as_ref()
                .and_then(|provenance| provenance.owner.as_ref())
                && owner != &resource.id
            {
                provenance_owned
                    .entry(owner.clone())
                    .or_default()
                    .insert(resource.id.clone());
            }
        }

        for (index, edge) in document.relationships.iter().enumerate() {
            outgoing.entry(edge.from.clone()).or_default().push(index);
            incoming.entry(edge.to.clone()).or_default().push(index);
        }

        Ok(Self {
            document,
            resources,
            names,
            incoming,
            outgoing,
            provenance_owned,
        })
    }

    pub(super) fn source(&self) -> GraphSource {
        GraphSource {
            schema: self.document.schema.clone(),
            framework_version: Some(self.document.framework_version.clone()),
            identity: self.document.identity.clone(),
            protocol: Some(self.document.protocol.clone()),
        }
    }

    pub(super) fn resource(&self, id: &str) -> &Resource {
        &self.document.resources[self.resources[id]]
    }

    pub(super) fn resource_ids(&self) -> BTreeSet<String> {
        self.resources.keys().cloned().collect()
    }

    pub(super) fn resolve_query_roots(
        &self,
        query: &GraphQuery,
    ) -> Result<ResolvedGraphRoots, GraphQueryError> {
        let mut roots = BTreeSet::new();
        let mut owner_roots = BTreeSet::new();

        for selector in &query.resources {
            roots.insert(self.resolve(selector, GraphSelectorKind::Resource, |_| true)?);
        }

        for selector in &query.contributors {
            let owner = self.resolve(selector, GraphSelectorKind::Contributor, |resource| {
                is_contributor(&resource.kind)
            })?;

            roots.insert(owner.clone());
            owner_roots.insert(owner);
        }

        for selector in &query.plugins {
            let owner = self.resolve(selector, GraphSelectorKind::Plugin, |resource| {
                resource.kind == ResourceKind::Plugin
            })?;

            roots.insert(owner.clone());
            owner_roots.insert(owner);
        }

        Ok(ResolvedGraphRoots { roots, owner_roots })
    }

    pub(super) fn resolve(
        &self,
        selector: &str,
        kind: GraphSelectorKind,
        eligible: impl Fn(&Resource) -> bool,
    ) -> Result<String, GraphQueryError> {
        if let Some(index) = self.resources.get(selector) {
            let resource = &self.document.resources[*index];

            if eligible(resource) {
                return Ok(resource.id.clone());
            }

            return Err(GraphQueryError::NotFound {
                kind,
                selector: selector.to_string(),
            });
        }

        let candidates = self
            .names
            .get(selector)
            .into_iter()
            .flatten()
            .filter(|id| eligible(self.resource(id)))
            .cloned()
            .collect::<Vec<_>>();

        match candidates.as_slice() {
            [] => Err(GraphQueryError::NotFound {
                kind,
                selector: selector.to_string(),
            }),
            [id] => Ok(id.clone()),
            _ => Err(GraphQueryError::Ambiguous {
                kind,
                selector: selector.to_string(),
                candidates,
            }),
        }
    }

    pub(super) fn expand_owner_roots(
        &self,
        roots: &BTreeSet<String>,
        owner_roots: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut seeds = roots.clone();
        let mut closure = BTreeSet::new();

        for owner in owner_roots {
            seeds.insert(owner.clone());
            closure.insert(owner.clone());

            if let Some(owned) = self.provenance_owned.get(owner) {
                seeds.extend(owned.iter().cloned());
                closure.extend(owned.iter().cloned());
            }
        }

        let mut pending = closure;

        while let Some(current) = pending.pop_first() {
            for edge in self.outgoing_edges(&current) {
                if matches!(
                    edge.kind,
                    RelationshipKind::Contains | RelationshipKind::Contributes
                ) && seeds.insert(edge.to.clone())
                {
                    pending.insert(edge.to.clone());
                }
            }
        }

        seeds
    }

    pub(super) fn traverse(
        &self,
        seeds: &BTreeSet<String>,
        family: GraphRelationFamily,
        direction: GraphDirection,
    ) -> BTreeSet<String> {
        let mut selected = seeds.clone();
        let mut pending = seeds.clone();

        while let Some(current) = pending.pop_first() {
            for edge in self.incident_edges(&current) {
                if !relation_matches(self, edge, family) {
                    continue;
                }

                for neighbor in traversal_neighbors(edge, &current, direction) {
                    if selected.insert(neighbor.to_string()) {
                        pending.insert(neighbor.to_string());
                    }
                }
            }
        }

        selected
    }

    pub(super) fn relationships_for(&self, id: &str, outgoing: bool) -> Vec<Relationship> {
        let relationships = if outgoing {
            self.outgoing.get(id)
        } else {
            self.incoming.get(id)
        };

        relationships
            .into_iter()
            .flatten()
            .map(|index| self.document.relationships[*index].clone())
            .collect()
    }

    /// The far endpoints of `id`'s `kind` relationships, in canonical relationship order.
    pub(super) fn neighbors(&self, id: &str, kind: RelationshipKind, outgoing: bool) -> Vec<&str> {
        let edges = if outgoing {
            self.outgoing.get(id)
        } else {
            self.incoming.get(id)
        };

        edges
            .into_iter()
            .flatten()
            .map(|index| &self.document.relationships[*index])
            .filter(|edge| edge.kind == kind)
            .map(|edge| {
                if outgoing {
                    edge.to.as_str()
                } else {
                    edge.from.as_str()
                }
            })
            .collect()
    }

    pub(super) fn relevant_cli_providers(&self, selected: &BTreeSet<String>) -> Vec<CliProvider> {
        let Some(cli) = &self.document.cli else {
            return Vec::new();
        };

        cli.providers
            .iter()
            .filter(|provider| {
                selected.contains(&provider.contributor)
                    || selected.contains(&upwell_tooling_schema::contribution_id(
                        &provider.contributor,
                        &provider.contribution,
                    ))
            })
            .cloned()
            .collect()
    }

    pub(super) fn cli_ownership(
        &self,
        resource: &Resource,
        providers: &[CliProvider],
    ) -> CliOwnershipSummary {
        super::explain::cli_ownership(&self.document, resource, providers)
    }

    fn incident_edges(&self, id: &str) -> Vec<&Relationship> {
        let mut indices = BTreeSet::new();

        indices.extend(self.incoming.get(id).into_iter().flatten().copied());
        indices.extend(self.outgoing.get(id).into_iter().flatten().copied());

        indices
            .into_iter()
            .map(|index| &self.document.relationships[index])
            .collect()
    }

    fn outgoing_edges(&self, id: &str) -> impl Iterator<Item = &Relationship> {
        self.outgoing
            .get(id)
            .into_iter()
            .flatten()
            .map(|index| &self.document.relationships[*index])
    }
}
