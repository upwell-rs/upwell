//! Aggregate reports, and plans that do not depend on input or enumeration order.

use super::*;

#[test]
fn aggregate_report_includes_independent_duplicate_and_dependency_failures() {
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            install("test/duplicate", early(0)),
            install("test/duplicate", early(1)),
            install_with(
                "test/consumer",
                early(2),
                [relation(
                    RelationKind::Requires,
                    plugin_target("test/missing"),
                )],
            ),
        ],
    )
    .expect_err("invalid graph fails");

    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::DuplicatePlugin { .. }))
    );
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::MissingDependency { .. }))
    );
}

#[test]
fn plans_and_diagnostics_are_invariant_under_all_input_permutations() {
    let router_slot = slot_id("test/router");
    let valid = vec![
        CompositionDirective::install(
            PluginDeclaration::new(plugin_id("test/default-router"), early(0))
                .provides(router_slot, SlotPolicy::Replaceable),
        ),
        CompositionDirective::replace(
            router_slot,
            PluginDeclaration::new(plugin_id("test/replacement"), early(1)),
        ),
        install_with(
            "test/consumer",
            early(2),
            [relation(
                RelationKind::Requires,
                RelationTarget::Slot(router_slot),
            )],
        ),
        install("test/independent", early(3)),
    ];
    let baseline = resolve_early_plugins(PROTOCOL, valid.clone()).expect("baseline resolves");

    for permutation in permutations(&valid) {
        assert_eq!(
            resolve_early_plugins(PROTOCOL, permutation).expect("permutation resolves"),
            baseline
        );
    }

    let invalid = vec![
        install_with(
            "test/alpha",
            early(0),
            [relation(
                RelationKind::Requires,
                plugin_target("test/missing"),
            )],
        ),
        install("test/duplicate", early(1)),
        install("test/duplicate", early(2)),
    ];
    let baseline = resolve_early_plugins(PROTOCOL, invalid.clone()).expect_err("baseline fails");

    for permutation in permutations(&invalid) {
        let diagnostics =
            resolve_early_plugins(PROTOCOL, permutation).expect_err("permutation fails");

        assert_eq!(diagnostics, baseline);
        assert_eq!(diagnostics.to_string(), baseline.to_string());
    }
}

#[test]
fn backend_shaped_enumeration_orders_produce_identical_plans() {
    let linkme = vec![
        install("test/alpha", early(10)),
        install_with(
            "test/gamma",
            early(30),
            [relation(RelationKind::After, plugin_target("test/beta"))],
        ),
        install("test/beta", early(20)),
    ];
    let inventory = linkme.iter().cloned().rev().collect::<Vec<_>>();

    assert_eq!(
        resolve_early_plugins(PROTOCOL, linkme).expect("linkme-shaped plan"),
        resolve_early_plugins(PROTOCOL, inventory).expect("inventory-shaped plan")
    );
}
