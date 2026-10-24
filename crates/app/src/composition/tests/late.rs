//! Late composition extends the early plan without mutating or reordering it.

use super::*;

#[test]
fn late_composition_extends_early_order_monotonically() {
    let early_plan = resolve_early_plugins(
        PROTOCOL,
        [
            install("test/zulu-early", early(0)),
            install("test/alpha-early", early(1)),
        ],
    )
    .expect("early plan resolves");
    let final_plan = extend_late_plugins(
        &early_plan,
        [
            install_with(
                "test/zulu-late",
                late(0),
                [relation(
                    RelationKind::Requires,
                    plugin_target("test/alpha-early"),
                )],
            ),
            install("test/alpha-late", late(1)),
        ],
    )
    .expect("late plan resolves");

    assert_eq!(
        ids(early_plan.plugins()),
        [plugin_id("test/alpha-early"), plugin_id("test/zulu-early")]
    );
    assert_eq!(
        ids(final_plan.plugins()),
        [
            plugin_id("test/alpha-early"),
            plugin_id("test/zulu-early"),
            plugin_id("test/alpha-late"),
            plugin_id("test/zulu-late"),
        ]
    );
}

#[test]
fn late_plugins_cannot_mutate_or_reorder_early_capabilities() {
    let slot = slot_id("test/router");
    let early_plan = resolve_early_plugins(
        PROTOCOL,
        [CompositionDirective::install(
            PluginDeclaration::new(plugin_id("test/router"), early(0))
                .provides(slot, SlotPolicy::Optional),
        )],
    )
    .expect("early plan resolves");
    let diagnostics = extend_late_plugins(
        &early_plan,
        [
            CompositionDirective::replace(
                slot,
                PluginDeclaration::new(plugin_id("test/late-router"), late(0)),
            ),
            install_with(
                "test/late-before",
                late(1),
                [relation(RelationKind::Before, plugin_target("test/router"))],
            ),
        ],
    )
    .expect_err("late mutations fail");

    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::LateSlotMutation { .. }))
    );
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::LateOrderingBeforeEarly { .. }))
    );
}

#[test]
fn late_plugins_cannot_reintroduce_an_early_suppressed_slot() {
    let slot = slot_id("test/openapi");
    let early_plan = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(
                PluginDeclaration::new(plugin_id("test/openapi"), early(0))
                    .provides(slot, SlotPolicy::Optional),
            ),
            CompositionDirective::suppress(slot, early(1)),
        ],
    )
    .expect("early slot suppresses");
    let diagnostics = extend_late_plugins(
        &early_plan,
        [CompositionDirective::install(
            PluginDeclaration::new(plugin_id("test/late-openapi"), late(0))
                .provides(slot, SlotPolicy::Optional),
        )],
    )
    .expect_err("late declaration cannot restore suppressed early slot");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::LateSlotMutation { .. }]
    ));
}

#[test]
fn early_conflicts_are_rechecked_against_late_installations() {
    let early_plan = resolve_early_plugins(
        PROTOCOL,
        [install_with(
            "test/early",
            early(0),
            [relation(
                RelationKind::Conflicts,
                plugin_target("test/late"),
            )],
        )],
    )
    .expect("absent early conflict target is allowed");
    let diagnostics = extend_late_plugins(&early_plan, [install("test/late", late(0))])
        .expect_err("late target activates early conflict");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::Conflict { .. }]
    ));
}

#[test]
fn early_after_relation_rejects_a_new_late_target() {
    let early_plan = resolve_early_plugins(
        PROTOCOL,
        [install_with(
            "test/early",
            early(0),
            [relation(RelationKind::After, plugin_target("test/late"))],
        )],
    )
    .expect("absent ordering target is allowed");
    let diagnostics = extend_late_plugins(&early_plan, [install("test/late", late(0))])
        .expect_err("late target cannot reorder early plan");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::EarlyOrderingAfterLate { .. }]
    ));
}
