//! Cycle diagnostics, and keeping structural errors from cascading into them.

use super::*;

#[test]
fn cycles_report_only_the_cyclic_component_with_typed_edges() {
    let directives = [
        install_with(
            "test/alpha",
            early(0),
            [
                relation(RelationKind::Requires, plugin_target("test/beta")),
                relation(RelationKind::Before, plugin_target("test/beta")),
            ],
        ),
        install("test/beta", early(1)),
        install_with(
            "test/downstream",
            early(2),
            [relation(
                RelationKind::Requires,
                plugin_target("test/alpha"),
            )],
        ),
    ];
    let diagnostics = resolve_early_plugins(PROTOCOL, directives).expect_err("cycle fails");
    let CompositionDiagnostic::Cycle {
        members,
        steps,
        display,
        ..
    } = &diagnostics.as_slice()[0]
    else {
        panic!("expected cycle diagnostic");
    };

    assert_eq!(display, "test/alpha -> test/beta -> test/alpha");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].plugin(), plugin_id("test/alpha"));
    assert_eq!(members[0].provenance(), early(0));
    assert_eq!(steps.len(), 2);
    assert!(
        steps
            .iter()
            .all(|step| step.from().plugin() != plugin_id("test/downstream"))
    );
    assert!(
        steps
            .iter()
            .any(|step| step.kinds().contains(&RelationKind::Requires))
    );
    assert!(
        steps
            .iter()
            .any(|step| step.kinds().contains(&RelationKind::Before))
    );
}

#[test]
fn cycle_members_include_every_plugin_in_the_cyclic_component() {
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            install_with(
                "test/alpha",
                early(0),
                [
                    relation(RelationKind::Before, plugin_target("test/beta")),
                    relation(RelationKind::After, plugin_target("test/beta")),
                ],
            ),
            install_with(
                "test/beta",
                early(1),
                [
                    relation(RelationKind::Before, plugin_target("test/gamma")),
                    relation(RelationKind::After, plugin_target("test/gamma")),
                ],
            ),
            install("test/gamma", early(2)),
        ],
    )
    .expect_err("strongly connected graph fails");
    let CompositionDiagnostic::Cycle { members, .. } = &diagnostics.as_slice()[0] else {
        panic!("expected cycle diagnostic");
    };

    assert_eq!(
        members
            .iter()
            .map(|member| member.plugin())
            .collect::<Vec<_>>(),
        [
            plugin_id("test/alpha"),
            plugin_id("test/beta"),
            plugin_id("test/gamma"),
        ]
    );
}

#[test]
fn structural_errors_do_not_emit_cascade_cycle_diagnostics() {
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            install_with(
                "test/alpha",
                early(0),
                [relation(RelationKind::Before, plugin_target("test/beta"))],
            ),
            install_with(
                "test/beta",
                early(1),
                [relation(RelationKind::Before, plugin_target("test/alpha"))],
            ),
            install("test/alpha", early(2)),
        ],
    )
    .expect_err("duplicate identity makes graph ambiguous");

    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::DuplicatePlugin { .. }))
    );
    assert!(
        !diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::Cycle { .. }))
    );
}

#[test]
fn competing_slot_directives_do_not_emit_cascade_cycles() {
    let slot = slot_id("test/router");
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(
                PluginDeclaration::new(plugin_id("test/default-router"), early(0))
                    .provides(slot, SlotPolicy::Replaceable)
                    .relates(relation(
                        RelationKind::Before,
                        plugin_target("test/consumer"),
                    )),
            ),
            CompositionDirective::replace(
                slot,
                PluginDeclaration::new(plugin_id("test/router-a"), early(1)),
            ),
            CompositionDirective::replace(
                slot,
                PluginDeclaration::new(plugin_id("test/router-b"), early(2)),
            ),
            install_with(
                "test/consumer",
                early(3),
                [relation(RelationKind::Before, slot_target("test/router"))],
            ),
        ],
    )
    .expect_err("competing replacements fail");

    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::MultipleReplacements { .. }))
    );
    assert!(
        !diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::Cycle { .. }))
    );
}

#[test]
fn duplicate_suppressions_do_not_hide_independent_cycles() {
    let slot = slot_id("test/openapi");
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(
                PluginDeclaration::new(plugin_id("test/openapi"), early(0))
                    .provides(slot, SlotPolicy::Optional),
            ),
            CompositionDirective::suppress(slot, early(1)),
            CompositionDirective::suppress(slot, early(2)),
            install_with(
                "test/alpha",
                early(3),
                [relation(RelationKind::Before, plugin_target("test/beta"))],
            ),
            install_with(
                "test/beta",
                early(4),
                [relation(RelationKind::Before, plugin_target("test/alpha"))],
            ),
        ],
    )
    .expect_err("independent failures are aggregated");

    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::MultipleSuppressions { .. }))
    );
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::Cycle { .. }))
    );
}

#[test]
fn duplicate_suppressions_remove_the_provider_before_cycle_analysis() {
    let slot = slot_id("test/openapi");
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(
                PluginDeclaration::new(plugin_id("test/openapi"), early(0))
                    .provides(slot, SlotPolicy::Optional)
                    .relates(relation(
                        RelationKind::Before,
                        plugin_target("test/consumer"),
                    )),
            ),
            CompositionDirective::suppress(slot, early(1)),
            CompositionDirective::suppress(slot, early(2)),
            install_with(
                "test/consumer",
                early(3),
                [relation(RelationKind::Before, slot_target("test/openapi"))],
            ),
        ],
    )
    .expect_err("duplicate suppressions remain diagnostic");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::MultipleSuppressions { .. }]
    ));
}

#[test]
fn unrelated_validation_errors_do_not_hide_independent_cycles() {
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            install_with(
                "test/consumer",
                early(0),
                [relation(
                    RelationKind::Requires,
                    plugin_target("test/missing"),
                )],
            ),
            install_with(
                "test/alpha",
                early(1),
                [relation(RelationKind::Before, plugin_target("test/beta"))],
            ),
            install_with(
                "test/beta",
                early(2),
                [relation(RelationKind::Before, plugin_target("test/alpha"))],
            ),
        ],
    )
    .expect_err("independent failures are aggregated");

    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::MissingDependency { .. }))
    );
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::Cycle { .. }))
    );
}

#[test]
fn deep_cycles_return_typed_diagnostics_without_recursive_traversal() {
    const NODE_COUNT: usize = 4_096;

    let ids: Vec<_> = (0..NODE_COUNT)
        .map(|index| {
            let text: &'static str = Box::leak(format!("deep/plugin-{index:04}").into_boxed_str());

            plugin_id(text)
        })
        .collect();
    let directives: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let target = ids[(index + 1) % NODE_COUNT];

            CompositionDirective::install(PluginDeclaration::new(*id, early(index as u32)).relates(
                relation(RelationKind::Before, RelationTarget::Plugin(target)),
            ))
        })
        .collect();
    let diagnostics = resolve_early_plugins(PROTOCOL, directives).expect_err("cycle fails");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::Cycle { members, .. }] if members.len() == NODE_COUNT
    ));
}
