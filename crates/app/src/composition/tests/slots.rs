//! Slot replacement and suppression, and duplicate and conflict diagnostics.

use super::*;

#[test]
fn replacement_preserves_implementation_and_slot_identity() {
    let router_slot = slot_id("test/router");
    let default = PluginDeclaration::new(plugin_id("test/default-router"), early(0))
        .provides(router_slot, SlotPolicy::Replaceable);
    let replacement = PluginDeclaration::new(plugin_id("acme/radix-router"), early(1));
    let consumer = PluginDeclaration::new(plugin_id("test/consumer"), early(2)).relates(relation(
        RelationKind::Requires,
        RelationTarget::Slot(router_slot),
    ));
    let plan = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(default),
            CompositionDirective::replace(router_slot, replacement),
            CompositionDirective::install(consumer),
        ],
    )
    .expect("replacement resolves");
    let decision = plan.replacements()[0];

    assert!(plan.plugin(plugin_id("test/default-router")).is_none());
    assert_eq!(
        plan.slot(router_slot).map(ResolvedPlugin::id),
        Some(plugin_id("acme/radix-router"))
    );
    assert_eq!(decision.replaced(), plugin_id("test/default-router"));
    assert_eq!(decision.replaced_provenance(), early(0));
    assert_eq!(decision.replacement(), plugin_id("acme/radix-router"));
    assert_eq!(
        ids(plan.plugins()),
        [plugin_id("acme/radix-router"), plugin_id("test/consumer")]
    );
}

#[test]
fn replacement_may_declare_the_slot_it_provides() {
    let router_slot = slot_id("test/router");
    let plan = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(
                PluginDeclaration::new(plugin_id("test/default-router"), early(0))
                    .provides(router_slot, SlotPolicy::Replaceable),
            ),
            CompositionDirective::replace(
                router_slot,
                PluginDeclaration::new(plugin_id("acme/router"), early(1))
                    .provides(router_slot, SlotPolicy::Replaceable),
            ),
        ],
    )
    .expect("matching replacement slot resolves");

    assert_eq!(
        plan.slot(router_slot).map(ResolvedPlugin::id),
        Some(plugin_id("acme/router"))
    );
}

#[test]
fn optional_slots_can_be_suppressed_but_fixed_slots_cannot_change() {
    let optional_slot = slot_id("test/openapi");
    let fixed_slot = slot_id("test/runtime");
    let optional = PluginDeclaration::new(plugin_id("test/openapi"), early(0))
        .provides(optional_slot, SlotPolicy::Optional);
    let fixed = PluginDeclaration::new(plugin_id("test/runtime"), early(1))
        .provides(fixed_slot, SlotPolicy::Fixed);
    let valid = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(optional.clone()),
            CompositionDirective::suppress(optional_slot, early(2)),
        ],
    )
    .expect("optional slot suppresses");
    let invalid_replacement = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(fixed.clone()),
            CompositionDirective::replace(
                fixed_slot,
                PluginDeclaration::new(plugin_id("test/other-runtime"), early(2)),
            ),
        ],
    )
    .expect_err("fixed slot rejects replacement");
    let invalid_suppression = resolve_early_plugins(
        PROTOCOL,
        [
            CompositionDirective::install(fixed),
            CompositionDirective::install(optional),
            CompositionDirective::suppress(fixed_slot, early(3)),
        ],
    )
    .expect_err("fixed slot rejects suppression");

    assert!(valid.slot(optional_slot).is_none());
    assert_eq!(
        valid.suppressions()[0].suppressed(),
        plugin_id("test/openapi")
    );
    assert_eq!(valid.suppressions()[0].suppressed_provenance(), early(0));
    assert!(
        invalid_replacement
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::ReplacementForbidden { .. }))
    );
    assert!(
        invalid_suppression
            .as_slice()
            .iter()
            .any(|item| matches!(item, CompositionDiagnostic::SuppressionForbidden { .. }))
    );
}

#[test]
fn duplicate_and_conflict_diagnostics_name_both_origins() {
    let duplicate = plugin_id("test/duplicate");
    let conflict_a = plugin_id("test/conflict-a");
    let conflict_b = plugin_id("test/conflict-b");
    let diagnostics = resolve_early_plugins(
        PROTOCOL,
        [
            install(duplicate.as_str(), early(9)),
            install(duplicate.as_str(), early(1)),
        ],
    )
    .expect_err("duplicate fails");
    let conflicts = resolve_early_plugins(
        PROTOCOL,
        [
            install_with(
                conflict_b.as_str(),
                early(1),
                [relation(
                    RelationKind::Conflicts,
                    RelationTarget::Plugin(conflict_a),
                )],
            ),
            install(conflict_a.as_str(), early(0)),
        ],
    )
    .expect_err("conflict fails");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::DuplicatePlugin { first, second, .. }]
            if first.ordinal() == 1 && second.ordinal() == 9
    ));
    assert!(matches!(
        conflicts.as_slice(),
        [CompositionDiagnostic::Conflict { left, right, .. }]
            if *left == conflict_a && *right == conflict_b
    ));
}
