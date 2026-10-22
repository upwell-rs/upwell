use semver::Version;
use serde::{Deserialize, Serialize};
use upwell_tooling_schema::{
    CliOwner, CliProvider, Diagnostic, DocumentIdentity, ProbeFailure, Relationship, Resource,
    ToolingDocument,
};

use super::error::{GraphEmitError, GraphQueryError};
use super::explain::explain_resource;
use super::failure::query_failure_graph;
use super::query::query_graph;

/// A semantic relationship family available to graph traversal.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum GraphRelationFamily {
    /// Every relationship, using each concrete family's semantic orientation.
    #[default]
    All,
    /// Runtime requirements, implementations, config bindings, condition gating, and
    /// construction ordering.
    Dependencies,
    /// Plugin requirements, slots, ordering, replacement, suppression, and conflicts.
    Composition,
    /// Scope topology and explicit scope placement relationships.
    Scopes,
    /// Hooks, lifecycle stages, and non-composition temporal ordering.
    Lifecycle,
    /// Containment, contribution, and validation relationships.
    Ownership,
}

/// Direction in which semantic graph relationships are traversed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum GraphDirection {
    /// Traverse toward both explanations and effects.
    #[default]
    Both,
    /// Traverse from selected resources toward resources that explain them.
    Upstream,
    /// Traverse from selected resources toward resources they affect.
    Downstream,
}

/// A deterministic graph request with repeated exact selectors.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphQuery {
    /// Exact resource stable IDs or exact case-sensitive resource names.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Exact contribution-owner stable IDs or exact case-sensitive names.
    #[serde(default)]
    pub contributors: Vec<String>,
    /// Exact plugin stable IDs or exact case-sensitive plugin names.
    #[serde(default)]
    pub plugins: Vec<String>,
    /// Semantic relationship family to emit and traverse.
    #[serde(default)]
    pub family: GraphRelationFamily,
    /// Semantic traversal direction from selected roots.
    #[serde(default)]
    pub direction: GraphDirection,
}

impl GraphQuery {
    /// Executes this request against a cloned, canonicalized, and validated document.
    pub fn execute(&self, document: &ToolingDocument) -> Result<GraphView, GraphQueryError> {
        query_graph(document, self)
    }

    /// Executes this request against diagnostic-only data from a failed preparation probe.
    pub fn execute_failure(
        &self,
        envelope_schema: Version,
        identity: &DocumentIdentity,
        failure: &ProbeFailure,
    ) -> Result<GraphView, GraphQueryError> {
        query_failure_graph(envelope_schema, identity, failure, self)
    }

    pub(super) fn has_selectors(&self) -> bool {
        !self.resources.is_empty() || !self.contributors.is_empty() || !self.plugins.is_empty()
    }
}

/// Source document metadata retained by graph-derived machine contracts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSource {
    /// Compatibility version of the source tooling document.
    pub schema: Version,
    /// Framework version that produced the source tooling document, when preparation exposed it.
    pub framework_version: Option<String>,
    /// Application and selected Cargo target identity retained by the probe envelope.
    pub identity: DocumentIdentity,
    /// Stable selected protocol identity, when preparation exposed it.
    pub protocol: Option<String>,
}

/// A separately versioned deterministic machine graph contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphView {
    /// Compatibility version of this graph-view contract.
    pub schema: Version,
    /// Metadata identifying the source tooling document.
    pub source: GraphSource,
    /// Whether this graph represents a complete successfully prepared tooling document.
    pub complete: bool,
    /// Lifecycle phase that prevented preparation, for an incomplete diagnostic graph.
    pub failure_phase: Option<String>,
    /// Semantic relationship family applied by the request.
    pub family: GraphRelationFamily,
    /// Semantic traversal direction applied by the request.
    pub direction: GraphDirection,
    /// Exact selector roots resolved to stable resource IDs.
    pub roots: Vec<String>,
    /// Selected resources in stable ID order.
    pub nodes: Vec<Resource>,
    /// Selected relationships in canonical source order.
    pub edges: Vec<Relationship>,
    /// Source diagnostics attached to at least one selected resource.
    pub diagnostics: Vec<Diagnostic>,
    /// CLI providers associated with selected contributor or contribution resources.
    pub cli_providers: Vec<CliProvider>,
}

impl GraphView {
    /// Emits compact canonical JSON for the graph-view machine contract.
    pub fn to_canonical_json(&self) -> Result<String, GraphEmitError> {
        let mut view = self.clone();

        view.roots.sort();
        view.roots.dedup();
        view.nodes.sort_by(|left, right| left.id.cmp(&right.id));
        view.edges.sort_by(relationship_cmp);
        view.diagnostics.sort_by(diagnostic_cmp);
        view.cli_providers
            .sort_by(|left, right| left.id.cmp(&right.id));

        Ok(serde_json::to_string(&view)?)
    }
}

/// One command ownership entry from the effective parser tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliCommandOwnership {
    /// Canonical command path from the parser root.
    pub path: Vec<String>,
    /// Optional framework command identity.
    pub id: Option<String>,
    /// Stable declaration owner.
    pub owner: CliOwner,
}

/// One argument ownership entry from the effective parser tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliArgumentOwnership {
    /// Canonical path of the command declaring the argument.
    pub command_path: Vec<String>,
    /// Stable argument identity within the command.
    pub id: String,
    /// Stable declaration owner.
    pub owner: CliOwner,
}

/// Deterministic parser command and argument ownership associated with an explanation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliOwnershipSummary {
    /// Commands owned by the selected resource or one of its CLI providers.
    pub commands: Vec<CliCommandOwnership>,
    /// Arguments owned by the selected resource or one of its CLI providers.
    pub arguments: Vec<CliArgumentOwnership>,
}

/// A deterministic explanation assembled from the same canonical graph index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceExplanation {
    /// Compatibility version of this explanation contract.
    pub schema: Version,
    /// Metadata identifying the source tooling document.
    pub source: GraphSource,
    /// Exact selector resolved to this resource.
    pub resource: Resource,
    /// Canonical relationships whose target is the selected resource.
    pub incoming: Vec<Relationship>,
    /// Canonical relationships whose source is the selected resource.
    pub outgoing: Vec<Relationship>,
    /// Diagnostics explicitly attached to the selected resource.
    pub diagnostics: Vec<Diagnostic>,
    /// CLI providers owned by the selected contributor or contribution resource.
    pub cli_providers: Vec<CliProvider>,
    /// Parser elements owned by those providers or by the selected application/framework owner.
    pub cli_ownership: CliOwnershipSummary,
}

impl ResourceExplanation {
    /// Builds an explanation for one exact stable ID or exact case-sensitive unique name.
    pub fn query(
        document: &ToolingDocument,
        selector: impl AsRef<str>,
    ) -> Result<Self, GraphQueryError> {
        explain_resource(document, selector.as_ref())
    }

    /// Emits compact canonical JSON for the resource-explanation machine contract.
    pub fn to_canonical_json(&self) -> Result<String, GraphEmitError> {
        let mut explanation = self.clone();

        explanation.incoming.sort_by(relationship_cmp);
        explanation.outgoing.sort_by(relationship_cmp);
        explanation.diagnostics.sort_by(diagnostic_cmp);
        explanation
            .cli_providers
            .sort_by(|left, right| left.id.cmp(&right.id));
        explanation
            .cli_ownership
            .commands
            .sort_by(|left, right| (&left.path, &left.id).cmp(&(&right.path, &right.id)));
        explanation.cli_ownership.arguments.sort_by(|left, right| {
            (&left.command_path, &left.id).cmp(&(&right.command_path, &right.id))
        });

        Ok(serde_json::to_string(&explanation)?)
    }
}

pub(super) fn relationship_cmp(left: &Relationship, right: &Relationship) -> std::cmp::Ordering {
    (&left.from, &left.kind, &left.to, &left.labels).cmp(&(
        &right.from,
        &right.kind,
        &right.to,
        &right.labels,
    ))
}

pub(super) fn diagnostic_cmp(left: &Diagnostic, right: &Diagnostic) -> std::cmp::Ordering {
    (
        &left.code,
        &left.severity,
        &left.message,
        &left.resources,
        &left.sources,
        &left.fix,
    )
        .cmp(&(
            &right.code,
            &right.severity,
            &right.message,
            &right.resources,
            &right.sources,
            &right.fix,
        ))
}
