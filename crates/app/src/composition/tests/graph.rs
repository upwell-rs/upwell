//! Dependency and ordering edges, and which relation targets are optional.

use super::*;

#[test]
fn dependency_and_ordering_edges_form_one_deterministic_graph() {
    let plan = resolve_early_plugins(
        PROTOCOL,
        [
            install_with(
                "test/gamma",
                early(2),
                [relation(RelationKind::After, plugin_target("test/alpha"))],
            ),
            install_with(
                "test/beta",
                early(1),
                [relation(
                    RelationKind::Requires,
                    plugin_target("test/gamma"),
                )],
            ),
            install("test/alpha", early(0)),
        ],
    )
    .expect("composition resolves");

    assert_eq!(
        ids(plan.plugins()),
        [
            plugin_id("test/alpha"),
            plugin_id("test/gamma"),
            plugin_id("test/beta"),
        ]
    );
}

#[test]
fn absent_order_and_conflict_targets_are_optional_but_dependencies_are_not() {
    let valid = resolve_early_plugins(
        PROTOCOL,
        [install_with(
            "test/source",
            early(0),
            [
                relation(RelationKind::Before, plugin_target("test/absent")),
                relation(RelationKind::Conflicts, slot_target("test/optional")),
            ],
        )],
    )
    .expect("optional relations ignore missing targets");
    let invalid = resolve_early_plugins(
        PROTOCOL,
        [install_with(
            "test/source",
            early(0),
            [relation(
                RelationKind::Requires,
                plugin_target("test/absent"),
            )],
        )],
    )
    .expect_err("hard dependency requires its target");

    assert_eq!(valid.plugins().len(), 1);
    assert!(matches!(
        invalid.as_slice(),
        [CompositionDiagnostic::MissingDependency { .. }]
    ));
}
