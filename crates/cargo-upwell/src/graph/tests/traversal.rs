//! Relation families, semantic orientation, and closure traversal.

use super::*;

#[test]
fn dependency_traversal_uses_semantic_orientation_and_excludes_scope_placement() {
    let mut document = document();
    let mut scope_edge = relationship(
        RelationshipKind::DependsOn,
        "component:consumer",
        "scope:request",
    );

    scope_edge
        .labels
        .insert(String::from("role"), String::from("scope"));
    document.resources.extend([
        resource("component:consumer", ResourceKind::Component, "Consumer"),
        resource("type:service", ResourceKind::Type, "Service"),
        resource(
            "provider:service",
            ResourceKind::Provider,
            "Service Provider",
        ),
        resource(
            "config-binding:service",
            ResourceKind::ConfigBinding,
            "Service Config",
        ),
        resource(
            "component:implementation",
            ResourceKind::Component,
            "Implementation",
        ),
        resource("scope:request", ResourceKind::Scope, "Request"),
    ]);
    document.relationships.extend([
        relationship(
            RelationshipKind::DependsOn,
            "component:consumer",
            "type:service",
        ),
        relationship(
            RelationshipKind::Provides,
            "provider:service",
            "type:service",
        ),
        relationship(
            RelationshipKind::Binds,
            "config-binding:service",
            "type:service",
        ),
        relationship(
            RelationshipKind::Provides,
            "component:implementation",
            "provider:service",
        ),
        scope_edge,
    ]);

    let upstream = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:consumer")],
            family: GraphRelationFamily::Dependencies,
            direction: GraphDirection::Upstream,
            ..GraphQuery::default()
        },
    )
    .expect("dependency graph resolves");

    assert_eq!(
        node_ids(&upstream),
        BTreeSet::from([
            "component:consumer",
            "component:implementation",
            "config-binding:service",
            "provider:service",
            "type:service",
        ])
    );
    assert!(!node_ids(&upstream).contains("scope:request"));

    let downstream = query_graph(
        &document,
        &GraphQuery {
            resources: vec![String::from("component:implementation")],
            family: GraphRelationFamily::Dependencies,
            direction: GraphDirection::Downstream,
            ..GraphQuery::default()
        },
    )
    .expect("reverse dependency graph resolves");

    assert!(node_ids(&downstream).contains("component:consumer"));
}

#[test]
fn contributor_and_plugin_selectors_include_owned_and_contained_closure() {
    let mut document = document();
    let mut owned = resource(
        "contribution:plugin:worker:feature",
        ResourceKind::Contribution,
        "feature",
    );
    let mut component = resource("component:worker", ResourceKind::Component, "Worker");

    owned.provenance = Some(provenance("plugin:worker"));
    component.provenance = Some(provenance("plugin:worker"));
    document.resources.extend([
        resource("plugin:worker", ResourceKind::Plugin, "Worker Plugin"),
        owned,
        resource(
            "contribution:plugin:worker:nested",
            ResourceKind::Contribution,
            "nested",
        ),
        component,
    ]);
    document.relationships.extend([
        relationship(
            RelationshipKind::Contains,
            "plugin:worker",
            "contribution:plugin:worker:feature",
        ),
        relationship(
            RelationshipKind::Contains,
            "contribution:plugin:worker:feature",
            "contribution:plugin:worker:nested",
        ),
        relationship(
            RelationshipKind::Contributes,
            "contribution:plugin:worker:nested",
            "component:worker",
        ),
    ]);

    let plugin_view = query_graph(
        &document,
        &GraphQuery {
            plugins: vec![String::from("Worker Plugin")],
            family: GraphRelationFamily::Ownership,
            direction: GraphDirection::Downstream,
            ..GraphQuery::default()
        },
    )
    .expect("plugin closure resolves");
    let contributor_view = query_graph(
        &document,
        &GraphQuery {
            contributors: vec![String::from("plugin:worker")],
            family: GraphRelationFamily::Ownership,
            direction: GraphDirection::Downstream,
            ..GraphQuery::default()
        },
    )
    .expect("contributor closure resolves");

    assert_eq!(node_ids(&plugin_view), node_ids(&contributor_view));
    assert_eq!(
        node_ids(&plugin_view),
        BTreeSet::from([
            "component:worker",
            "contribution:plugin:worker:feature",
            "contribution:plugin:worker:nested",
            "plugin:worker",
        ])
    );
}

#[test]
fn traversal_is_iterative_cycle_safe_and_conflicts_are_symmetric() {
    let mut document = document();

    document.resources.extend([
        resource("plugin:a", ResourceKind::Plugin, "A"),
        resource("plugin:b", ResourceKind::Plugin, "B"),
        resource("plugin:c", ResourceKind::Plugin, "C"),
    ]);
    document.relationships.extend([
        relationship(RelationshipKind::DependsOn, "plugin:a", "plugin:b"),
        relationship(RelationshipKind::DependsOn, "plugin:b", "plugin:a"),
        relationship(RelationshipKind::Conflicts, "plugin:b", "plugin:c"),
    ]);

    for direction in [GraphDirection::Upstream, GraphDirection::Downstream] {
        let view = query_graph(
            &document,
            &GraphQuery {
                resources: vec![String::from("plugin:c")],
                family: GraphRelationFamily::Composition,
                direction,
                ..GraphQuery::default()
            },
        )
        .expect("cyclic composition graph terminates");

        assert_eq!(
            node_ids(&view),
            BTreeSet::from(["plugin:a", "plugin:b", "plugin:c"])
        );
    }
}

#[test]
fn scope_lifecycle_and_ownership_families_follow_documented_orientation() {
    let mut document = document();

    document.resources.extend([
        resource("scope:root", ResourceKind::Scope, "Root"),
        resource("scope:request", ResourceKind::Scope, "Request"),
        resource("component:worker", ResourceKind::Component, "Worker"),
        resource("hook:startup", ResourceKind::Hook, "Startup Hook"),
        resource("lifecycle:startup", ResourceKind::Lifecycle, "Startup"),
        resource("lifecycle:serve", ResourceKind::Lifecycle, "Serve"),
        resource("plugin:worker", ResourceKind::Plugin, "Worker Plugin"),
        resource(
            "contribution:plugin:worker:route",
            ResourceKind::Contribution,
            "route",
        ),
    ]);
    document.relationships.extend([
        relationship(RelationshipKind::OpensScope, "scope:root", "scope:request"),
        relationship(RelationshipKind::Hooks, "component:worker", "hook:startup"),
        relationship(RelationshipKind::Hooks, "hook:startup", "lifecycle:startup"),
        relationship(
            RelationshipKind::OrdersBefore,
            "lifecycle:startup",
            "lifecycle:serve",
        ),
        relationship(
            RelationshipKind::Contains,
            "plugin:worker",
            "contribution:plugin:worker:route",
        ),
    ]);

    for (root, family, expected) in [
        ("scope:request", GraphRelationFamily::Scopes, "scope:root"),
        (
            "lifecycle:serve",
            GraphRelationFamily::Lifecycle,
            "component:worker",
        ),
        (
            "contribution:plugin:worker:route",
            GraphRelationFamily::Ownership,
            "plugin:worker",
        ),
    ] {
        let view = query_graph(
            &document,
            &GraphQuery {
                resources: vec![root.to_string()],
                family,
                direction: GraphDirection::Upstream,
                ..GraphQuery::default()
            },
        )
        .expect("family traversal resolves");

        assert!(node_ids(&view).contains(expected));
    }
}
