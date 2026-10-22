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
