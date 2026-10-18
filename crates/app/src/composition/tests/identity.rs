//! Stable identifiers, framework-namespace claims and declaration identity.

use super::*;

#[test]
fn stable_ids_validate_namespaced_ascii_paths() {
    let valid = PluginId::new("acme/rpc.router-v2").expect("valid id");
    let missing = PluginId::new("router").expect_err("namespace is required");
    let uppercase = PluginId::new("Acme/router").expect_err("uppercase is rejected");
    let empty = PluginId::new("acme//router").expect_err("empty segment is rejected");
    let unicode = PluginId::new("acme/routér").expect_err("unicode is rejected");

    assert_eq!(valid.as_str(), "acme/rpc.router-v2");
    assert_eq!(valid.namespace(), "acme");
    assert_eq!(valid.name(), "rpc.router-v2");
    assert_eq!(valid.local_path(), "rpc.router-v2");
    assert_eq!(missing.category(), "plugin");
    assert_eq!(valid.to_string(), "acme/rpc.router-v2");
    assert_eq!(missing.kind(), IdErrorKind::MissingNamespace);
    assert_eq!(uppercase.kind(), IdErrorKind::InvalidSegmentStart);
    assert_eq!(empty.kind(), IdErrorKind::MissingNamespace);
    assert_eq!(unicode.kind(), IdErrorKind::InvalidCharacter);
}

#[test]
fn application_declarations_cannot_claim_framework_namespace() {
    let diagnostics = resolve_early_plugins(PROTOCOL, [install("upwell/router", early(0))])
        .expect_err("framework namespace is reserved");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::ReservedNamespace { .. }]
    ));
}

#[test]
fn protocol_provenance_must_match_the_selected_protocol() {
    let other = protocol_id("test/other-protocol");
    let provenance = InstallationProvenance::new(InstallationOrigin::ProtocolDefault(other), 0);
    let diagnostics = resolve_early_plugins(PROTOCOL, [install("test/plugin", provenance)])
        .expect_err("foreign protocol provenance is rejected");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::ProtocolOriginMismatch {
            selected: PROTOCOL,
            declared,
            ..
        }] if *declared == other
    ));
}

#[test]
fn protocol_plugins_cannot_claim_framework_namespace() {
    let provenance =
        InstallationProvenance::new(InstallationOrigin::ProtocolMandatory(PROTOCOL), 0);
    let diagnostics = resolve_early_plugins(PROTOCOL, [install("upwell/forged", provenance)])
        .expect_err("protocol plugin cannot forge framework ownership");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::ReservedNamespace { .. }]
    ));
}

#[test]
fn public_declarations_cannot_claim_framework_slots() {
    let slot = slot_id("upwell/forged-slot");
    let directive = CompositionDirective::install(
        PluginDeclaration::new(plugin_id("test/plugin"), early(0))
            .provides(slot, SlotPolicy::Replaceable),
    );
    let diagnostics = resolve_early_plugins(PROTOCOL, [directive])
        .expect_err("framework slot namespace is reserved");

    assert!(matches!(
        diagnostics.as_slice(),
        [CompositionDiagnostic::ReservedSlotNamespace { .. }]
    ));
}

#[test]
fn public_directives_cannot_target_framework_slots() {
    let slot = slot_id("upwell/forged-slot");
    let replacement = CompositionDirective::replace(
        slot,
        PluginDeclaration::new(plugin_id("test/replacement"), early(0)),
    );
    let suppression = CompositionDirective::suppress(slot, early(1));
    let diagnostics = resolve_early_plugins(PROTOCOL, [replacement, suppression])
        .expect_err("framework slot targets are reserved");

    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.as_slice().iter().all(|diagnostic| matches!(
        diagnostic,
        CompositionDiagnostic::ReservedSlotNamespace { .. }
    )));
}

#[test]
fn identifiers_are_category_safe_and_lexically_ordered() {
    let alpha = plugin_id("test/alpha");
    let beta = plugin_id("test/beta");
    let protocol = PROTOCOL;

    assert!(alpha < beta);
    assert_eq!(protocol.as_str(), "test/protocol");
    assert_eq!(slot_id("test/alpha").as_str(), alpha.as_str());
}

#[test]
fn contribution_identity_is_local_to_explicit_contributor() {
    let id = contribution_id("test/routes");
    let app = ContributionProvenance::new(Contributor::Application, id);
    let plugin = ContributionProvenance::new(Contributor::Plugin(plugin_id("test/router")), id);

    assert_ne!(app, plugin);
    assert_eq!(app.contribution(), plugin.contribution());
    assert_eq!(
        plugin.contributor(),
        Contributor::Plugin(plugin_id("test/router"))
    );
}

#[test]
fn relation_order_and_duplicates_do_not_change_declaration_identity() {
    let before = relation(RelationKind::Before, plugin_target("test/beta"));
    let requires = relation(RelationKind::Requires, slot_target("test/router"));
    let left = PluginDeclaration::new(plugin_id("test/alpha"), early(0))
        .with_relations([requires, before, requires]);
    let right = PluginDeclaration::new(plugin_id("test/alpha"), early(0))
        .relates(before)
        .relates(requires);

    assert_eq!(left, right);
    assert_eq!(left.relations(), &[requires, before]);
}

#[test]
fn independent_plugins_use_lexical_identity_order() {
    let plan = resolve_early_plugins(
        PROTOCOL,
        [
            install("test/zulu", early(0)),
            install("test/alpha", early(2)),
            install("test/middle", early(1)),
        ],
    )
    .expect("composition resolves");

    assert_eq!(
        ids(plan.plugins()),
        [
            plugin_id("test/alpha"),
            plugin_id("test/middle"),
            plugin_id("test/zulu"),
        ]
    );
}
