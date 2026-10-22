//! Exact selector resolution, ambiguity, and CLI-provider explanations.

use super::*;

#[test]
fn exact_selectors_use_id_precedence_and_report_sorted_name_ambiguity() {
    let mut document = document();

    document.resources.extend([
        resource("component:id-wins", ResourceKind::Component, "Shared"),
        resource("component:a", ResourceKind::Component, "Shared"),
        resource(
            "component:name-collision",
            ResourceKind::Component,
            "component:id-wins",
        ),
    ]);

    let view = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:id-wins")],
            ..GraphQuery::default()
        },
    )
    .expect("stable ID selects before an equal name");

    assert_eq!(view.roots, ["component:id-wins"]);
    assert_eq!(node_ids(&view), BTreeSet::from(["component:id-wins"]));

    let error = explain_resource(&document, "Shared").expect_err("duplicate name is ambiguous");
    let GraphQueryError::Ambiguous {
        kind,
        selector,
        candidates,
    } = error
    else {
        panic!("unexpected selector result");
    };

    assert_eq!(kind, GraphSelectorKind::Resource);
    assert_eq!(selector, "Shared");
    assert_eq!(candidates, ["component:a", "component:id-wins"]);
}

#[test]
fn exact_unique_name_and_not_found_resolution_are_typed() {
    let mut document = document();

    document.resources.push(resource(
        "component:worker",
        ResourceKind::Component,
        "Worker",
    ));

    let explanation = ResourceExplanation::query(&document, "Worker").expect("name selects");

    assert_eq!(explanation.resource.id, "component:worker");

    let error = explain_resource(&document, "worker").expect_err("matching is case-sensitive");

    assert!(matches!(
        error,
        GraphQueryError::NotFound {
            kind: GraphSelectorKind::Resource,
            selector,
        } if selector == "worker"
    ));
}

#[test]
fn commas_and_hostile_selector_strings_remain_whole_exact_values() {
    let mut document = document();

    document.resources.extend([
        resource("component:a,b", ResourceKind::Component, "Comma, Name"),
        resource("component:a", ResourceKind::Component, "A"),
        resource("component:b", ResourceKind::Component, "B"),
    ]);

    for selector in ["component:a,b", "Comma, Name"] {
        let explanation = explain_resource(&document, selector).expect("whole selector resolves");

        assert_eq!(explanation.resource.id, "component:a,b");
    }

    for selector in ["*", "component:a,b\ncomponent:b", "../component:a", ""] {
        let error = explain_resource(&document, selector).expect_err("selector stays exact");

        assert!(matches!(error, GraphQueryError::NotFound { .. }));
    }
}

#[test]
fn explanation_reports_cli_providers_and_parser_ownership_without_winner_inference() {
    let document = rich_document();
    let contributor = explain_resource(&document, "plugin:worker").expect("plugin explains");
    let contribution = explain_resource(&document, "worker-cli").expect("contribution explains");

    for explanation in [&contributor, &contribution] {
        assert_eq!(explanation.cli_providers.len(), 1);
        assert_eq!(
            explanation.cli_providers[0].id,
            "cli-provider:plugin:worker:worker-cli"
        );
        assert_eq!(explanation.cli_ownership.commands.len(), 1);
        assert_eq!(
            explanation.cli_ownership.commands[0].path,
            ["fixture", "worker"]
        );
        assert_eq!(explanation.cli_ownership.arguments.len(), 1);
        assert_eq!(explanation.cli_ownership.arguments[0].id, "threads");
    }

    let json = contributor
        .to_canonical_json()
        .expect("explanation serializes");

    assert!(!json.contains("winner"));
    assert!(json.contains("resolved-provider"));
}

#[test]
fn cli_provider_ownership_requires_the_exact_canonical_contribution_id() {
    let mut document = rich_document();
    let mut collision = resource(
        "contribution:plugin:worker:not-worker-cli",
        ResourceKind::Contribution,
        "worker-cli",
    );

    collision.provenance = Some(provenance("plugin:worker"));
    document.resources.push(collision);

    let exact = explain_resource(&document, "contribution:plugin:worker:worker-cli")
        .expect("canonical contribution explains");
    let collision = explain_resource(&document, "contribution:plugin:worker:not-worker-cli")
        .expect("colliding contribution explains");

    assert_eq!(exact.cli_providers.len(), 1);
    assert!(collision.cli_providers.is_empty());
    assert!(collision.cli_ownership.commands.is_empty());
    assert!(collision.cli_ownership.arguments.is_empty());
}
