//! Typed failure envelopes and their redacted, identity-preserving diagnostics.

use super::*;

#[test]
fn typed_config_failures_emit_safe_diagnostics_without_source_display() {
    let error = crate::Error::Config(upwell_config::ConfigError::Substitution {
        path: String::from("service.token"),
        source: upwell_config::TemplateError::Bare(upwell_config::TemplateErrorKind::Message(
            String::from("resolved secret=probe-secret"),
        )),
    });
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Configure,
        error,
    ))
    .failure();
    let envelope = ProbeEnvelope::failure(probe_identity(), failure);
    let json = envelope.to_json().expect("typed failure serializes");

    assert!(json.contains("upwell/tooling-config-substitution"));
    assert!(json.contains("config:service.token"));
    assert!(!json.contains("probe-secret"));
}

#[test]
fn arbitrary_setup_and_plugin_error_displays_never_enter_failure_envelopes() {
    let secret = "secret bearer-token=probe-secret";
    let setup = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Setup,
        std::io::Error::other(secret),
    ))
    .failure();
    let plugin = ToolingProbeError::PluginCatalog(crate::Error::Other(Box::new(
        std::io::Error::other(secret),
    )))
    .failure();

    for failure in [setup, plugin] {
        let json = ProbeEnvelope::failure(probe_identity(), failure)
            .to_json()
            .expect("typed failure serializes");

        assert!(!json.contains("probe-secret"));
        assert!(!json.contains("bearer-token"));
    }
}

#[test]
fn plugin_composition_failures_preserve_stable_plugin_and_installation_identities() {
    let provenance = InstallationProvenance::new(InstallationOrigin::ApplicationDeclaration, 2);
    let error = crate::Error::Composition(CompositionDiagnostics::new(vec![
        CompositionDiagnostic::MissingDependency {
            plugin: PluginId::new("fixture/plugin").expect("valid plugin ID"),
            provenance,
            target: RelationTarget::Slot(
                PluginSlotId::new("fixture/database").expect("valid slot ID"),
            ),
        },
    ]));
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Configure,
        error,
    ))
    .failure();

    assert_eq!(failure.diagnostics.len(), 1);
    assert_eq!(
        failure.diagnostics[0].resources,
        [
            "plugin:fixture/plugin",
            "plugin-installation:application-declaration:2",
            "plugin-slot:fixture/database",
        ]
    );
}

#[test]
fn di_failures_use_stable_component_and_underlying_type_identities() {
    let error = crate::Error::Di(upwell_di::Error::MissingDependency {
        component: String::from("Friendly Worker Name"),
        component_id: String::from("worker-component"),
        dependency: String::from("fixture::Service (qualifier `primary`)"),
        type_name: String::from("fixture::Service"),
    });
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();

    assert_eq!(
        failure.diagnostics[0].resources,
        ["component:worker-component", "type:fixture::Service"]
    );
    assert!(
        !failure.diagnostics[0]
            .resources
            .iter()
            .any(|resource| resource.contains("qualifier"))
    );
}

#[test]
fn orphan_provider_failures_emit_provider_component_and_type_identities() {
    let error = crate::Error::Di(upwell_di::Error::ProviderComponentMissing(Box::new(
        upwell_di::ProviderComponentMissing {
            trait_name: String::from("Friendly Service"),
            trait_type: String::from("fixture::Service"),
            component: String::from("Friendly Provider"),
            component_type: String::from("fixture::Provider"),
            qualifier: String::from("primary"),
        },
    )));
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-provider-component-missing");
    assert_eq!(
        diagnostic.resources,
        [
            "provider:fixture::Service:fixture::Provider:primary",
            "component:fixture::Provider",
            "type:fixture::Service",
            "type:fixture::Provider",
        ]
    );
    assert_eq!(
        diagnostic.fix.as_deref(),
        Some(
            "Register the provider's concrete component or remove the orphan provider descriptor."
        )
    );
}

#[test]
fn scope_violation_diagnostics_include_component_type_and_scope_identities() {
    let consumer_scope =
        upwell_core::ScopeId::new("fixture/request").expect("valid consumer scope identity");
    let dependency_scope =
        upwell_core::ScopeId::new("fixture/connection").expect("valid dependency scope identity");
    let error = crate::Error::Di(upwell_di::Error::ScopeViolation(Box::new(
        upwell_di::ScopeViolation {
            component: String::from("Friendly Consumer"),
            component_id: String::from("consumer-component"),
            dependency: String::from("Friendly Dependency"),
            dependency_type: String::from("fixture::Dependency"),
            component_scope: "Request",
            component_scope_id: consumer_scope,
            dependency_scope: "Connection",
            dependency_scope_id: dependency_scope,
        },
    )));
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();

    assert_eq!(
        failure.diagnostics[0].resources,
        [
            "component:consumer-component",
            "type:fixture::Dependency",
            "scope:fixture/request",
            "scope:fixture/connection",
        ]
    );
}

#[test]
fn scope_unreachable_diagnostics_include_consumer_and_candidate_provider_identities() {
    let consumer_scope =
        upwell_core::ScopeId::new("fixture/request").expect("valid consumer scope identity");
    let provider_scope =
        upwell_core::ScopeId::new("fixture/sibling").expect("valid provider scope identity");
    let error = crate::Error::Di(upwell_di::Error::ScopeUnreachableDependency(Box::new(
        upwell_di::ScopeUnreachableDependency {
            component: String::from("Friendly Consumer"),
            component_id: String::from("consumer-component"),
            dependency: String::from("Friendly Service"),
            dependency_type: String::from("fixture::Service"),
            component_scope: String::from("Request"),
            component_scope_id: consumer_scope,
            providers: vec![upwell_di::ScopeUnreachableProvider {
                component: String::from("Friendly Provider"),
                component_id: String::from("provider-component"),
                component_type: String::from("fixture::Provider"),
                scope: String::from("Sibling"),
                scope_id: provider_scope,
                qualifier: String::from("primary"),
            }],
        },
    )));
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-scope-unreachable");
    assert_eq!(
        diagnostic.resources,
        [
            "component:consumer-component",
            "type:fixture::Service",
            "scope:fixture/request",
            "provider:fixture::Service:fixture::Provider:primary",
            "component:provider-component",
            "type:fixture::Provider",
            "scope:fixture/sibling",
        ]
    );
    assert_eq!(
        diagnostic.fix.as_deref(),
        Some(
            "Move a provider into the consumer's scope chain or move the consumer beneath a provider scope."
        )
    );
}

#[test]
fn actual_prepare_failure_preserves_component_id_distinct_from_name() {
    let result = App::<DiagnosticFailureProtocol>::builder("diagnostic-failure")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&DIAGNOSTIC_CONSUMER)
        .component_descriptor(&DIAGNOSTIC_DEPENDENCY)
        .prepare();
    let error = match result {
        Ok(_) => panic!("sibling scope dependency is rejected"),
        Err(error) => error,
    };
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let resources = &failure.diagnostics[0].resources;

    assert!(resources.contains(&String::from("component:stable-consumer-id")));
    assert!(!resources.contains(&String::from("component:Friendly Consumer Name")));
    assert!(resources.contains(&format!(
        "type:{}",
        std::any::type_name::<DiagnosticDependency>()
    )));
    assert!(resources.contains(&String::from("scope:test/diagnostic-consumer")));
    assert!(resources.contains(&String::from("scope:test/diagnostic-dependency")));
}

#[test]
fn actual_prepare_failure_classifies_sibling_trait_provider_as_scope_unreachable() {
    let result = App::<DiagnosticProviderFailureProtocol>::builder("diagnostic-provider-failure")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&DIAGNOSTIC_PROVIDER_CONSUMER)
        .component_descriptor(&DIAGNOSTIC_PROVIDER_COMPONENT)
        .prepare();
    let error = match result {
        Ok(_) => panic!("sibling provider is unreachable"),
        Err(error) => error,
    };
    let failure = ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-scope-unreachable");
    assert!(
        diagnostic
            .resources
            .contains(&String::from("component:stable-provider-consumer-id"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("component:stable-provider-component-id"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("scope:test/diagnostic-consumer"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("scope:test/diagnostic-dependency"))
    );
    assert!(diagnostic.resources.contains(&format!(
        "type:{}",
        std::any::type_name::<dyn DiagnosticProvider>()
    )));
    assert!(diagnostic.resources.iter().any(|resource| {
        resource.starts_with("provider:")
            && resource.contains(std::any::type_name::<dyn DiagnosticProvider>())
            && resource.contains(std::any::type_name::<DiagnosticProviderComponent>())
            && resource.ends_with(":sibling")
    }));
}

#[test]
fn invalid_envelope_does_not_open_or_truncate_response_path() {
    let fixture = TempFixture::new("upwell-app-invalid-before-open");
    let path = fixture.child("response.json");
    let mut envelope = ProbeEnvelope::failure(
        probe_identity(),
        upwell_tooling_schema::ProbeFailure {
            phase: None,
            diagnostics: vec![upwell_tooling_schema::Diagnostic {
                code: String::from("invalid"),
                severity: upwell_tooling_schema::DiagnosticSeverity::Error,
                message: String::from("invalid fixture"),
                ..upwell_tooling_schema::Diagnostic::default()
            }],
            resource_kinds: Default::default(),
        },
    );

    std::fs::write(&path, "preserve-me").expect("fixture response exists");
    envelope.identity.application.clear();

    assert!(emit_probe_envelope(&path, &envelope).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).expect("fixture response remains readable"),
        "preserve-me"
    );
}
