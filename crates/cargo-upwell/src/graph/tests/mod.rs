use std::collections::{BTreeMap, BTreeSet};

use upwell_tooling_schema::{
    CliArgument, CliCommand, CliMetadata, CliOwner, CliProvider, CliProviderKind, Diagnostic,
    DiagnosticSeverity, DocumentIdentity, Provenance, Relationship, RelationshipKind, Resource,
    ResourceKind, ToolingDocument,
};

use super::{
    GraphDirection, GraphQuery, GraphQueryError, GraphRelationFamily, GraphSelectorKind,
    ResourceExplanation, explain_resource, query_graph,
};

mod output;
mod selectors;
mod traversal;

fn rich_document() -> ToolingDocument {
    let mut document = document();
    let mut contribution = resource(
        "contribution:plugin:worker:worker-cli",
        ResourceKind::Contribution,
        "worker-cli",
    );
    let mut component = resource("component:worker", ResourceKind::Component, "Worker");
    let provider_id = "cli-provider:plugin:worker:worker-cli";

    contribution.provenance = Some(provenance("plugin:worker"));
    component.provenance = Some(provenance("plugin:worker"));
    document.resources.extend([
        resource("plugin:worker", ResourceKind::Plugin, "Worker Plugin"),
        contribution,
        component,
        resource("type:worker", ResourceKind::Type, "Worker Type"),
    ]);
    document.relationships.extend([
        relationship_with_labels(
            RelationshipKind::Contributes,
            "plugin:worker",
            "contribution:plugin:worker:worker-cli",
            [(String::from("role"), String::from("resolved-provider"))],
        ),
        relationship(
            RelationshipKind::Contributes,
            "contribution:plugin:worker:worker-cli",
            "component:worker",
        ),
        relationship(
            RelationshipKind::Provides,
            "component:worker",
            "type:worker",
        ),
    ]);
    document.diagnostics.extend([
        Diagnostic {
            code: String::from("fixture/unrelated"),
            severity: DiagnosticSeverity::Info,
            message: String::from("unrelated"),
            resources: vec![String::from("application")],
            ..Diagnostic::default()
        },
        Diagnostic {
            code: String::from("fixture/worker-warning"),
            severity: DiagnosticSeverity::Warning,
            message: String::from("worker warning"),
            resources: vec![String::from("component:worker")],
            ..Diagnostic::default()
        },
    ]);
    document.cli = Some(CliMetadata {
        root: CliCommand {
            name: String::from("fixture"),
            owner: CliOwner::Application,
            commands: vec![CliCommand {
                name: String::from("worker"),
                owner: CliOwner::Plugin {
                    provider: String::from(provider_id),
                },
                arguments: vec![CliArgument {
                    id: String::from("threads"),
                    long: Some(String::from("threads")),
                    owner: CliOwner::Plugin {
                        provider: String::from(provider_id),
                    },
                    ..CliArgument::default()
                }],
                ..CliCommand::default()
            }],
            ..CliCommand::default()
        },
        providers: vec![CliProvider {
            id: String::from(provider_id),
            contributor: String::from("plugin:worker"),
            contribution: String::from("worker-cli"),
            kind: CliProviderKind::Command,
        }],
        default_command: None,
    });

    document
}

fn document() -> ToolingDocument {
    let mut document = ToolingDocument::new(
        "0.20.0",
        DocumentIdentity {
            application: String::from("fixture"),
            ..DocumentIdentity::default()
        },
        "test/protocol",
    );

    document.resources.push(resource(
        "application",
        ResourceKind::Application,
        "fixture",
    ));

    document
}

fn resource(id: &str, kind: ResourceKind, name: &str) -> Resource {
    Resource {
        id: id.to_string(),
        kind,
        name: name.to_string(),
        ..Resource::default()
    }
}

fn relationship(kind: RelationshipKind, from: &str, to: &str) -> Relationship {
    Relationship {
        kind,
        from: from.to_string(),
        to: to.to_string(),
        labels: BTreeMap::new(),
    }
}

fn relationship_with_labels(
    kind: RelationshipKind,
    from: &str,
    to: &str,
    labels: impl IntoIterator<Item = (String, String)>,
) -> Relationship {
    Relationship {
        labels: labels.into_iter().collect(),
        ..relationship(kind, from, to)
    }
}

fn provenance(owner: &str) -> Provenance {
    Provenance {
        owner: Some(owner.to_string()),
        ..Provenance::default()
    }
}

fn node_ids(view: &super::GraphView) -> BTreeSet<&str> {
    view.nodes
        .iter()
        .map(|resource| resource.id.as_str())
        .collect()
}
