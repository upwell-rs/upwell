use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde_json::json;

use crate::{
    BinaryTargetIdentity, CliArgument, CliCardinality, CliCommand, CliMetadata, CliOwner,
    CliProvider, CliProviderKind, Diagnostic, DiagnosticSeverity, DocumentIdentity, Facet,
    PackageIdentity, ProbeEnvelope, ProbeFailure, ProbeOutcome, ProbeValidationError, Provenance,
    Relationship, RelationshipKind, Resource, ResourceDisplay, ResourceKind, SourceLocation,
    TOOLING_SCHEMA_VERSION, ToolingDocument, ValidationError, ValidationResult, cli_provider_id,
    contribution_id, diagnostic_resource_kind, parse_cli_provider_id, parse_contribution_id,
};

mod cli;
mod diagnostic;
mod document;
mod probe;
mod version;

fn fixture() -> ToolingDocument {
    let mut document = ToolingDocument::new(
        "0.20.0",
        DocumentIdentity {
            application: String::from("fixture"),
            ..DocumentIdentity::default()
        },
        "test/protocol",
    );

    document.resources = vec![
        Resource {
            id: String::from("type:test::Worker"),
            kind: ResourceKind::Type,
            name: String::from("Worker"),
            ..Resource::default()
        },
        Resource {
            id: String::from("component:worker"),
            kind: ResourceKind::Component,
            name: String::from("Worker"),
            ..Resource::default()
        },
    ];
    document.relationships.push(Relationship {
        kind: RelationshipKind::Provides,
        from: String::from("component:worker"),
        to: String::from("type:test::Worker"),
        labels: BTreeMap::new(),
    });
    document.facets.insert(
        String::from("third-party/routes"),
        Facet {
            schema_version: 2,
            value: json!({"routes": [{"method": "GET", "path": "/health"}]}),
        },
    );

    document
}

fn probe_identity() -> DocumentIdentity {
    DocumentIdentity {
        application: String::from("fixture"),
        package: Some(PackageIdentity {
            name: String::from("fixture-package"),
            version: Some(String::from("1.2.3")),
            manifest_path: Some(String::from("/workspace/fixture/Cargo.toml")),
        }),
        binary: Some(BinaryTargetIdentity {
            name: String::from("fixture-bin"),
        }),
        source: Some(SourceLocation {
            file: String::from("src/main.rs"),
            line: Some(17),
            column: Some(5),
            rust_path: None,
        }),
        workspace_root: None,
    }
}

fn failure_fixture() -> ProbeFailure {
    ProbeFailure {
        phase: Some(String::from("prepare")),
        diagnostics: vec![Diagnostic {
            code: String::from("upwell/tooling-prepare"),
            message: String::from("Application planning failed."),
            severity: DiagnosticSeverity::Error,
            resources: Vec::new(),
            sources: Vec::new(),
            fix: None,
        }],
        resource_kinds: BTreeMap::new(),
    }
}

fn cli_fixture() -> CliMetadata {
    CliMetadata {
        root: CliCommand {
            name: String::from("fixture"),
            arguments: vec![
                CliArgument {
                    id: String::from("input"),
                    index: Some(1),
                    aliases: vec![String::from("z"), String::from("a")],
                    visible_aliases: vec![String::from("visible")],
                    value_names: vec![String::from("FIRST"), String::from("REST")],
                    cardinality: CliCardinality {
                        min_values: 1,
                        max_values: None,
                        repeatable: true,
                    },
                    owner: CliOwner::Plugin {
                        provider: String::from("cli-provider:plugin:test/plugin:test/args"),
                    },
                    ..CliArgument::default()
                },
                CliArgument {
                    id: String::from("profile"),
                    long: Some(String::from("profile")),
                    owner: CliOwner::Framework,
                    ..CliArgument::default()
                },
            ],
            commands: vec![CliCommand {
                name: String::from("inspect"),
                owner: CliOwner::Application,
                ..CliCommand::default()
            }],
            owner: CliOwner::Application,
            ..CliCommand::default()
        },
        default_command: None,
        providers: vec![CliProvider {
            id: String::from("cli-provider:plugin:test/plugin:test/args"),
            contributor: String::from("plugin:test/plugin"),
            contribution: String::from("test/args"),
            kind: CliProviderKind::Args,
        }],
    }
}

fn add_cli_resources(document: &mut ToolingDocument) {
    document.resources.push(Resource {
        id: String::from("plugin:test/plugin"),
        kind: ResourceKind::Plugin,
        name: String::from("test/plugin"),
        ..Resource::default()
    });
}
