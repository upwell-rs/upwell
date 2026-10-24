//! Condition gating: a component's conditions explain it upstream in the dependency family.

use super::*;

#[test]
fn conditions_gate_components_upstream_in_the_dependency_family() {
    let mut document = document();
    let mut input = relationship(
        RelationshipKind::DependsOn,
        "condition:custom:when",
        "component:default",
    );

    input
        .labels
        .insert(String::from("role"), String::from("eligibility-input"));
    document.resources.extend([
        resource("component:custom", ResourceKind::Component, "Custom"),
        resource("component:default", ResourceKind::Component, "Default"),
        resource("condition:custom:when", ResourceKind::Condition, "when"),
    ]);
    document.relationships.extend([
        relationship(
            RelationshipKind::Gates,
            "condition:custom:when",
            "component:custom",
        ),
        input,
    ]);

    let upstream = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:custom")],
            family: GraphRelationFamily::Dependencies,
            direction: GraphDirection::Upstream,
            ..GraphQuery::default()
        },
    )
    .expect("the component selects");
    let downstream = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:default")],
            family: GraphRelationFamily::Dependencies,
            direction: GraphDirection::Downstream,
            ..GraphQuery::default()
        },
    )
    .expect("the input selects");

    assert_eq!(
        node_ids(&upstream),
        BTreeSet::from([
            "component:custom",
            "component:default",
            "condition:custom:when"
        ])
    );
    assert_eq!(node_ids(&downstream), node_ids(&upstream));
}

/// A trait with a flag-gated custom provider and an unconditional fallback, labelled as
/// the probe labels them.
fn selection_document() -> ToolingDocument {
    let mut document = document();
    let labelled = |id: &str, kind: ResourceKind, name: &str, labels: &[(&str, &str)]| {
        let mut resource = resource(id, kind, name);

        resource.labels = labels
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();

        resource
    };

    document.resources.extend([
        resource("type:dyn Auth", ResourceKind::Type, "Auth"),
        labelled(
            "component:custom",
            ResourceKind::Component,
            "Custom",
            &[("condition-state", "fails")],
        ),
        labelled(
            "component:default",
            ResourceKind::Component,
            "Default",
            &[("condition-state", "unconditional")],
        ),
        labelled(
            "provider:custom",
            ResourceKind::Provider,
            "custom",
            &[("effective", "false"), ("selection-role", "conditional")],
        ),
        labelled(
            "provider:default",
            ResourceKind::Provider,
            "default",
            &[("effective", "true"), ("selection-role", "fallback")],
        ),
        labelled(
            "condition:custom:enabled",
            ResourceKind::Condition,
            "enabled",
            &[("predicate", "not"), ("outcome", "fails")],
        ),
        labelled(
            "condition:custom:enabled/flag",
            ResourceKind::Condition,
            "flag",
            &[
                ("predicate", "config-bool"),
                ("outcome", "holds"),
                ("fact", "Auth@auth#custom"),
            ],
        ),
    ]);
    document.relationships.extend([
        relationship(
            RelationshipKind::Provides,
            "provider:custom",
            "type:dyn Auth",
        ),
        relationship(
            RelationshipKind::Provides,
            "provider:default",
            "type:dyn Auth",
        ),
        relationship(
            RelationshipKind::Provides,
            "component:custom",
            "provider:custom",
        ),
        relationship(
            RelationshipKind::Provides,
            "component:default",
            "provider:default",
        ),
        relationship(
            RelationshipKind::Gates,
            "condition:custom:enabled",
            "component:custom",
        ),
        relationship(
            RelationshipKind::Gates,
            "condition:custom:enabled/flag",
            "condition:custom:enabled",
        ),
    ]);

    document
}

#[test]
fn explaining_a_trait_shows_every_mapping_and_its_condition_path() {
    let document = selection_document();

    let explanation = explain_resource(&document, "type:dyn Auth").expect("the trait explains");

    let summary = explanation
        .selection
        .iter()
        .map(|selection| {
            (
                selection.provider.as_str(),
                selection.effective,
                selection.role.as_deref(),
                selection.condition_state.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let custom = explanation
        .selection
        .iter()
        .find(|selection| selection.provider == "provider:custom")
        .expect("the custom mapping is listed");
    let path = custom
        .conditions
        .iter()
        .map(|step| (step.id.as_str(), step.depth, step.outcome.as_deref()))
        .collect::<Vec<_>>();

    assert_eq!(
        summary,
        [
            ("provider:custom", false, Some("conditional"), Some("fails")),
            (
                "provider:default",
                true,
                Some("fallback"),
                Some("unconditional")
            ),
        ]
    );
    assert_eq!(
        path,
        [
            ("condition:custom:enabled", 0, Some("fails")),
            ("condition:custom:enabled/flag", 1, Some("holds")),
        ]
    );
    assert_eq!(
        custom.conditions[1].detail.as_deref(),
        Some("Auth@auth#custom")
    );
}

#[test]
fn explaining_a_provider_component_lists_its_trait_siblings() {
    let document = selection_document();

    let explanation =
        explain_resource(&document, "component:custom").expect("the component explains");

    let providers = explanation
        .selection
        .iter()
        .map(|selection| selection.provider.as_str())
        .collect::<Vec<_>>();

    assert_eq!(providers, ["provider:custom", "provider:default"]);
}

#[test]
fn unrelated_resources_have_no_selection_and_omit_it_from_json() {
    let document = selection_document();

    let explanation = explain_resource(&document, "application").expect("the application explains");
    let json = explanation
        .to_canonical_json()
        .expect("the explanation serializes");

    assert!(explanation.selection.is_empty());
    assert!(!json.contains("\"selection\""));
    assert!(!json.contains("\"transition\""));
}
