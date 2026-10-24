//! Unfiltered graphs, diagnostics filtering, and canonical output.

use super::*;

#[test]
fn no_selectors_returns_disconnected_full_graph_with_filtered_edges() {
    let mut document = document();

    document.resources.extend([
        resource("component:a", ResourceKind::Component, "A"),
        resource("type:a", ResourceKind::Type, "A Type"),
        resource("component:isolated", ResourceKind::Component, "Isolated"),
        resource("scope:root", ResourceKind::Scope, "Root"),
    ]);
    document.relationships.extend([
        relationship(RelationshipKind::Provides, "component:a", "type:a"),
        relationship(
            RelationshipKind::DependsOn,
            "component:isolated",
            "scope:root",
        ),
    ]);
    document.relationships[1]
        .labels
        .insert(String::from("role"), String::from("scope"));
    document.diagnostics.extend([
        Diagnostic {
            code: String::from("fixture/unattributed"),
            severity: DiagnosticSeverity::Warning,
            message: String::from("global validation context"),
            ..Diagnostic::default()
        },
        Diagnostic {
            code: String::from("fixture/component-a"),
            severity: DiagnosticSeverity::Info,
            message: String::from("component context"),
            resources: vec![String::from("component:a")],
            ..Diagnostic::default()
        },
    ]);

    let view = GraphQuery {
        family: GraphRelationFamily::Dependencies,
        ..GraphQuery::default()
    }
    .execute(&document)
    .expect("full graph resolves");

    assert_eq!(view.nodes.len(), document.resources.len());
    assert_eq!(view.edges.len(), 1);
    assert_eq!(view.diagnostics.len(), 2);
    assert!(view.roots.is_empty());
}

#[test]
fn filtered_successful_graph_retains_selected_and_unattributed_diagnostics() {
    let mut document = rich_document();

    document.diagnostics.push(Diagnostic {
        code: String::from("fixture/global-context"),
        severity: DiagnosticSeverity::Info,
        message: String::from("global validation context"),
        ..Diagnostic::default()
    });

    let view = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:worker")],
            ..GraphQuery::default()
        },
    )
    .expect("filtered graph resolves");
    let codes = view
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();

    assert_eq!(codes, ["fixture/global-context", "fixture/worker-warning"]);
}

#[test]
fn filtered_diagnostics_only_reference_resources_visible_in_the_result() {
    let mut document = rich_document();

    document.diagnostics.push(Diagnostic {
        code: String::from("fixture/cross-resource"),
        severity: DiagnosticSeverity::Warning,
        message: String::from("worker and application context"),
        resources: vec![
            String::from("component:worker"),
            String::from("application"),
        ],
        ..Diagnostic::default()
    });

    let view = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:worker")],
            ..GraphQuery::default()
        },
    )
    .expect("filtered graph resolves");
    let diagnostic = view
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "fixture/cross-resource")
        .expect("diagnostic attached to the selected resource is retained");

    assert!(!node_ids(&view).contains("application"));
    assert_eq!(diagnostic.resources, ["component:worker"]);

    let explanation = explain_resource(&document, "component:worker").expect("resource explains");
    let diagnostic = explanation
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "fixture/cross-resource")
        .expect("diagnostic attached to the explained resource is retained");

    assert_eq!(diagnostic.resources, ["component:worker"]);
}

#[test]
fn canonical_output_is_identical_under_input_permutations() {
    let mut first = rich_document();
    let mut second = first.clone();

    second.resources.reverse();
    second.relationships.reverse();
    second.diagnostics.reverse();
    second
        .cli
        .as_mut()
        .expect("CLI fixture exists")
        .providers
        .reverse();

    let query = GraphQuery {
        contributors: vec![String::from("plugin:worker")],
        ..GraphQuery::default()
    };
    let first_json = query
        .execute(&first)
        .expect("first graph resolves")
        .to_canonical_json()
        .expect("first graph serializes");
    let second_json = query
        .execute(&second)
        .expect("second graph resolves")
        .to_canonical_json()
        .expect("second graph serializes");

    assert_eq!(first_json, second_json);

    first.canonicalize();
    assert!(first_json.contains(&format!(
        "\"schema\":\"{}\"",
        super::super::TOOLING_SCHEMA_VERSION
    )));
}

#[test]
fn every_emitted_edge_has_both_endpoint_nodes_and_diagnostics_are_relevant() {
    let document = rich_document();
    let view = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:worker")],
            direction: GraphDirection::Both,
            ..GraphQuery::default()
        },
    )
    .expect("selected graph resolves");
    let ids = node_ids(&view);

    assert!(
        view.edges
            .iter()
            .all(|edge| ids.contains(edge.from.as_str()) && ids.contains(edge.to.as_str()))
    );
    assert_eq!(view.diagnostics.len(), 1);
    assert_eq!(view.diagnostics[0].code, "fixture/worker-warning");
}

#[test]
fn successful_graph_is_explicitly_complete_with_available_source_facts() {
    let view = query_graph(&document(), &GraphQuery::default()).expect("graph resolves");

    assert!(view.complete);
    assert_eq!(view.failure_phase, None);
    assert_eq!(view.source.framework_version.as_deref(), Some("0.20.0"));
    assert_eq!(view.source.protocol.as_deref(), Some("test/protocol"));
}
