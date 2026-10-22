use cargo_upwell::{
    CliArgumentOwnership, CliCommandOwnership, CliOwnershipSummary, ConditionStep, GraphSource,
    ProviderSelection, ResolvedConsumer, ResourceExplanation,
};
use semver::Version;
use serde_json::json;
use upwell_tooling_schema::{
    CliOwner, CliProvider, CliProviderKind, Diagnostic, DiagnosticSeverity, DocumentIdentity,
    Facet, PreviewAction, PreviewChangedBinding, PreviewComponent, PreviewDemand,
    PreviewEvaluation, PreviewOutcome, PreviewReason, PreviewReasonKind, PreviewRole,
    PreviewStrategy, Provenance, Relationship, RelationshipKind, Resource, ResourceKind,
    TransitionPreview,
};

use super::write_explanation;
use crate::cli::ExplainFormat;

#[test]
fn text_explanation_includes_generic_details_resolutions_and_cli_ownership() {
    let explanation = fixture();
    let mut output = Vec::new();

    write_explanation(&explanation, ExplainFormat::Text, false, &mut output)
        .expect("explanation writes");

    let output = String::from_utf8(output).expect("explanation is UTF-8");

    for expected in [
        "id: component:worker",
        "kind: component",
        "owner: plugin:worker",
        "role: resolved-provider",
        "third-party/data@7: object (2 fields)",
        "warning[fixture/warning]",
        "cli-provider:worker [command]",
        "command fixture worker [cli-provider:worker]",
        "argument fixture worker threads [cli-provider:worker]",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in {output}"
        );
    }
}

#[test]
fn declarative_display_extends_text_explanation() {
    let mut explanation = fixture();

    explanation.resource.display = Some(upwell_tooling_schema::ResourceDisplay {
        label: Some(String::from("rendered explanation")),
        group: Some(String::from("HTTP routes")),
        summary: Some(String::from("owner summary")),
        details: std::collections::BTreeMap::from([(
            String::from("route"),
            String::from("GET /health"),
        )]),
    });

    let mut output = Vec::new();

    write_explanation(&explanation, ExplainFormat::Text, false, &mut output)
        .expect("explanation writes");

    let output = String::from_utf8(output).expect("explanation is UTF-8");

    assert!(output.contains("name: rendered explanation"));
    assert!(output.contains("Display"));
    assert!(output.contains("route: GET /health"));
}

#[test]
fn json_explanation_uses_the_library_machine_contract() {
    let explanation = fixture();
    let mut output = Vec::new();

    write_explanation(&explanation, ExplainFormat::Json, true, &mut output)
        .expect("JSON explanation writes");

    assert_eq!(
        String::from_utf8(output).expect("JSON is UTF-8"),
        format!(
            "{}\n",
            explanation
                .to_canonical_json()
                .expect("explanation serializes")
        )
    );
}

#[test]
fn text_explanation_escapes_controls_in_details_and_cli_ownership() {
    let mut explanation = fixture();

    explanation.resource.name = String::from("Worker\u{1b}[31m\nnext\rline");
    explanation.resource.provenance.as_mut().unwrap().owner =
        Some(String::from("plugin\u{1b}\nowner"));
    explanation.cli_ownership.arguments[0].id = String::from("threads\r\n\u{1b}");
    let mut output = Vec::new();

    write_explanation(&explanation, ExplainFormat::Text, false, &mut output)
        .expect("explanation writes");

    let output = String::from_utf8(output).expect("explanation is UTF-8");

    assert!(!output.contains('\u{1b}'));
    assert!(!output.contains('\r'));
    assert!(output.contains(r"name: Worker\u{1b}[31m\nnext\rline"));
    assert!(output.contains(r"owner: plugin\u{1b}\nowner"));
    assert!(output.contains(r"threads\r\n\u{1b}"));
}

#[test]
fn text_explanation_shows_provider_selection_and_the_transition_preview() {
    let mut explanation = fixture();

    explanation.selection = vec![ProviderSelection {
        provider: String::from("provider:dyn Auth:Custom:dyn Auth"),
        component: Some(String::from("component:custom-auth")),
        effective: true,
        role: Some(String::from("conditional")),
        condition_state: Some(String::from("holds")),
        conditions: vec![ConditionStep {
            id: String::from("condition:custom-auth:enabled"),
            depth: 0,
            predicate: Some(String::from("config-bool")),
            outcome: Some(String::from("holds")),
            detail: Some(String::from("Auth@auth#custom")),
        }],
        resolved_by: vec![ResolvedConsumer {
            consumer: String::from("component:worker"),
            reason: Some(String::from("primary-provider-in-winning-set")),
        }],
    }];
    explanation.transition = Some(TransitionPreview {
        assignments: vec![String::from("auth.custom")],
        outcome: PreviewOutcome::Planned,
        evaluation: PreviewEvaluation::Complete,
        changed: vec![PreviewChangedBinding {
            path: String::from("auth"),
            type_name: String::from("Auth"),
        }],
        eligibility: Vec::new(),
        components: vec![PreviewComponent {
            component: String::from("worker"),
            factory: Some(String::from("static")),
            role: PreviewRole::Singleton,
            action: PreviewAction::Replace,
            strategy: Some(PreviewStrategy::Reconstruct),
            reasons: vec![PreviewReason {
                kind: PreviewReasonKind::DependencyTargetChanged,
                path: vec![PreviewDemand {
                    consumer: String::from("worker"),
                    ordinal: 0,
                    requested: Some(String::from("dyn Auth")),
                }],
            }],
        }],
        displaced: vec![String::from("worker")],
    });

    let mut output = Vec::new();

    write_explanation(&explanation, ExplainFormat::Text, false, &mut output)
        .expect("explanation writes");

    let output = String::from_utf8(output).expect("explanation is UTF-8");

    for expected in [
        "provider:dyn Auth:Custom:dyn Auth [effective] role=conditional condition=holds",
        "resolved by component:worker (primary-provider-in-winning-set)",
        "condition:custom-auth:enabled config-bool -> holds (Auth@auth#custom)",
        "assignments: auth.custom",
        "outcome: planned",
        "changed auth (Auth)",
        "*replace worker [singleton] strategy=reconstruct",
        "dependency_target_changed: worker#0 (dyn Auth)",
        "displaced: worker",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in {output}"
        );
    }
}

fn fixture() -> ResourceExplanation {
    let provider = String::from("cli-provider:worker");

    ResourceExplanation {
        schema: Version::new(0, 20, 0),
        source: GraphSource {
            schema: Version::new(0, 20, 0),
            framework_version: Some(String::from("0.20.0")),
            identity: DocumentIdentity {
                application: String::from("fixture"),
                ..DocumentIdentity::default()
            },
            protocol: Some(String::from("fixture/protocol")),
        },
        resource: Resource {
            id: String::from("component:worker"),
            kind: ResourceKind::Component,
            name: String::from("Worker"),
            display: None,
            provenance: Some(Provenance {
                owner: Some(String::from("plugin:worker")),
                origin: Some(String::from("plugin-contribution")),
                ..Provenance::default()
            }),
            labels: [(String::from("scope"), String::from("request"))]
                .into_iter()
                .collect(),
            facets: [(
                String::from("third-party/data"),
                Facet {
                    schema_version: 7,
                    value: json!({"alpha": true, "beta": [1, 2]}),
                },
            )]
            .into_iter()
            .collect(),
        },
        incoming: Vec::new(),
        outgoing: vec![Relationship {
            kind: RelationshipKind::DependsOn,
            from: String::from("component:worker"),
            to: String::from("provider:worker"),
            labels: [
                (String::from("role"), String::from("resolved-provider")),
                (String::from("selection-reason"), String::from("primary")),
            ]
            .into_iter()
            .collect(),
        }],
        diagnostics: vec![Diagnostic {
            code: String::from("fixture/warning"),
            severity: DiagnosticSeverity::Warning,
            message: String::from("worker warning"),
            resources: vec![String::from("component:worker")],
            ..Diagnostic::default()
        }],
        cli_providers: vec![CliProvider {
            id: provider.clone(),
            contributor: String::from("plugin:worker"),
            contribution: String::from("worker-cli"),
            kind: CliProviderKind::Command,
        }],
        cli_ownership: CliOwnershipSummary {
            commands: vec![CliCommandOwnership {
                path: vec![String::from("fixture"), String::from("worker")],
                id: Some(String::from("worker")),
                owner: CliOwner::Plugin {
                    provider: provider.clone(),
                },
            }],
            arguments: vec![CliArgumentOwnership {
                command_path: vec![String::from("fixture"), String::from("worker")],
                id: String::from("threads"),
                owner: CliOwner::Plugin { provider },
            }],
        },
        selection: Vec::new(),
        transition: None,
    }
}
