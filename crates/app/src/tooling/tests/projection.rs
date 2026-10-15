//! Prepared projection stability, redaction, provenance and ordering.

use super::*;

#[test]
fn prepared_projection_is_read_only_stable_and_redacted() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    FACTORY_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    DEPENDENCY_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    HOOK_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    HOOK_DEPENDENCY_CALLS.store(0, Ordering::SeqCst);
    PROJECTION_CALLBACK_POISONED.store(false, Ordering::SeqCst);

    let prepared = App::<()>::builder("tooling-projection")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&SNAPSHOT_COMPONENT)
        .prepare()
        .expect("application prepares");
    let prepared_callback_counts = descriptor_callback_counts();

    PROJECTION_CALLBACK_POISONED.store(true, Ordering::SeqCst);

    let first = prepared
        .tooling_document()
        .expect("prepared state projects");
    let second = prepared
        .tooling_document()
        .expect("projection can be read repeatedly");

    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    assert!(prepared_callback_counts.iter().all(|count| *count > 0));
    assert_eq!(descriptor_callback_counts(), prepared_callback_counts);
    assert_eq!(
        first
            .to_canonical_json()
            .expect("first document serializes"),
        second
            .to_canonical_json()
            .expect("second document serializes")
    );
    assert!(first.resources.iter().any(|resource| {
        resource.id == "component:projected-component"
            && resource.labels["rust-type"].ends_with("ProjectedComponent")
            && resource.labels["construction"] == "planned-factory"
            && resource.labels["factory-selection"] == "default"
            && resource.labels["factory-candidate-count"] == "1"
            && resource.labels["factory-explicit-count"] == "0"
    }));
    assert!(first.resources.iter().any(|resource| {
        resource
            .id
            .starts_with("hook:projected-component:config_reload:")
    }));
    assert!(first.relationships.iter().any(|relationship| {
        relationship.from == "component:projected-component"
            && relationship.labels.get("name") == Some(&String::from("SnapshotDependencyA"))
    }));
    assert!(
        !first
            .to_canonical_json()
            .expect("document serializes")
            .contains("TypeId")
    );
    PROJECTION_CALLBACK_POISONED.store(false, Ordering::SeqCst);
}

fn descriptor_callback_counts() -> [usize; 4] {
    [
        FACTORY_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        DEPENDENCY_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        HOOK_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        HOOK_DEPENDENCY_CALLS.load(Ordering::SeqCst),
    ]
}

#[test]
fn displaced_plugin_contribution_points_to_the_final_registry_resource() {
    let document = App::<DisplacingProtocol>::builder("displaced-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<DisplacedComponentPlugin>()
        .prepare()
        .expect("application prepares")
        .tooling_document()
        .expect("prepared state projects");
    let contribution = document
        .resources
        .iter()
        .find(|resource| {
            resource.id == "contribution:plugin:test/displaced-component:test/projected-component"
        })
        .expect("contribution resource exists");
    assert_eq!(contribution.labels["decision"], "displaced");
    assert_eq!(
        contribution.labels["requested-target"],
        "component:projected-component"
    );
    assert_eq!(
        contribution.labels["effective-target"],
        "component:protocol-projected-component"
    );
    assert!(!contribution.labels.contains_key("applied-target"));
    assert!(!document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contributes
            && relationship.from == contribution.id
            && relationship.to == "component:protocol-projected-component"
    }));
}

#[test]
fn replacement_and_suppression_decisions_project_explicitly() {
    let replacement = App::<ProjectionPluginProtocol>::builder("replacement")
        .config_source(ConfigManager::<Toml>::empty())
        .with_plugin_declarations(|plugins| {
            plugins.replace(PROJECTION_SLOT, ProjectionReplacementPlugin);
        })
        .prepare()
        .expect("replacement prepares")
        .tooling_document()
        .expect("replacement projects");
    let suppression = App::<ProjectionPluginProtocol>::builder("suppression")
        .config_source(ConfigManager::<Toml>::empty())
        .with_plugin_declarations(|plugins| plugins.suppress(PROJECTION_SLOT))
        .prepare()
        .expect("suppression prepares")
        .tooling_document()
        .expect("suppression projects");

    assert!(replacement.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Replaces
            && relationship.from == "plugin:test/projection-replacement"
            && relationship.to == "plugin:test/projection-default"
    }));
    assert!(suppression.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Suppresses
            && relationship.to == "plugin:test/projection-default"
    }));
    assert!(suppression.resources.iter().any(|resource| {
        resource.id == "plugin:test/projection-default"
            && resource.labels["decision"] == "suppressed"
    }));
    assert!(replacement.resources.iter().any(|resource| {
        resource.id == "plugin:test/projection-replacement"
            && resource.labels["effective"] == "true"
    }));
    assert!(
        suppression
            .resources
            .iter()
            .any(|resource| { resource.id == "plugin-slot:test/projection-slot" })
    );
    assert!(suppression.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "suppression:test/projection-slot"
            && relationship.to == "plugin-slot:test/projection-slot"
            && relationship.labels["role"] == "suppressed-slot"
    }));
}

#[test]
fn selected_plugin_component_and_provider_retain_provenance_and_resolution() {
    let document = App::<()>::builder("provider-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<ProviderContributionPlugin>()
        .component_descriptor(&PROVIDER_CONSUMER_COMPONENT)
        .prepare()
        .expect("provider contribution prepares")
        .tooling_document()
        .expect("provider contribution projects");
    let component = document
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("contributed component exists");
    let provider = document
        .resources
        .iter()
        .find(|resource| {
            resource.kind == upwell_tooling_schema::ResourceKind::Provider
                && resource.labels.get("qualifier") == Some(&String::from("projected"))
        })
        .expect("contributed provider exists");

    assert_eq!(
        component
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/provider-contribution")
    );
    assert_eq!(
        provider
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/provider-contribution")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "component:provider-consumer"
            && relationship.to == provider.id
            && relationship.labels["role"] == "resolved-provider"
            && relationship.labels["selection-reason"] == "qualified"
            && relationship.labels["resolution"] == "eager"
            && relationship.labels["observation"] == "snapshot"
            && relationship.labels["cardinality"] == "one"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "component:provider-consumer"
            && relationship.to.starts_with("type:")
            && !relationship.labels.contains_key("role")
    }));
}

#[test]
#[cfg(feature = "cli")]
fn cli_providers_project_as_owned_contribution_resources() {
    let protocol = ProtocolPluginRegistrar::new(<() as ProtocolDefinition>::ID);
    let mut application = crate::ApplicationPluginRegistrar::new();

    application.register::<CliContributionPlugin>();

    let mut catalog = crate::EarlyPluginCatalog::resolve(protocol, application)
        .expect("CLI plugin catalog resolves");
    let command = clap::Command::new("cli-contribution");
    let framework = clap::Command::new("cli-contribution");

    catalog
        .augment_cli(command, framework, &[], false)
        .expect("CLI provider composes");

    let document = App::<()>::builder("cli-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .with_early_plugin_catalog(catalog)
        .prepare()
        .expect("CLI contribution prepares")
        .tooling_document()
        .expect("CLI contribution projects");
    let id = "contribution:plugin:test/cli-contribution:test/cli-args";
    let contribution = document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .expect("CLI contribution resource exists");

    assert_eq!(
        contribution.kind,
        upwell_tooling_schema::ResourceKind::Contribution
    );
    assert_eq!(contribution.labels["provider-kind"], "args");
    assert_eq!(
        contribution
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/cli-contribution")
    );
    assert_eq!(
        contribution
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.origin.as_deref()),
        Some("cli-provider")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contributes
            && relationship.from == "plugin:test/cli-contribution"
            && relationship.to == id
    }));
}

#[test]
fn tooling_feature_projects_without_cli() {
    let prepared = App::<()>::builder("tooling-without-cli")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("CLI-independent application prepares");
    let document = prepared
        .tooling_document()
        .expect("CLI-independent state projects");

    assert_eq!(document.protocol, "upwell/none");
    assert!(document.cli.is_none());
}

#[test]
fn construction_ordinals_preserve_exact_independent_declaration_order() {
    let first = App::<()>::builder("permuted")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&COMPONENT)
        .component_descriptor(&ALTERNATE_COMPONENT)
        .prepare()
        .expect("first declaration order prepares")
        .tooling_document()
        .expect("first declaration order projects");
    let second = App::<()>::builder("permuted")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&ALTERNATE_COMPONENT)
        .component_descriptor(&COMPONENT)
        .prepare()
        .expect("second declaration order prepares")
        .tooling_document()
        .expect("second declaration order projects");

    let first_projected = first
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("projected component exists");
    let first_alternate = first
        .resources
        .iter()
        .find(|resource| resource.id == "component:alternate-component")
        .expect("alternate component exists");
    let second_projected = second
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("projected component exists");
    let second_alternate = second
        .resources
        .iter()
        .find(|resource| resource.id == "component:alternate-component")
        .expect("alternate component exists");

    assert_eq!(first_projected.labels["plan-ordinal"], "0");
    assert_eq!(first_alternate.labels["plan-ordinal"], "1");
    assert_eq!(second_alternate.labels["plan-ordinal"], "0");
    assert_eq!(second_projected.labels["plan-ordinal"], "1");
    assert!(first.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "component:projected-component"
            && relationship.to == "component:alternate-component"
            && relationship.labels["scope"] == Singleton::ID.as_str()
    }));
}

#[test]
fn plugin_relations_and_owner_qualified_facets_project_without_dropped_endpoints() {
    let document = App::<()>::builder("plugin-relations")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<RelationPlugin>()
        .register_plugin::<AlternateFacetPlugin>()
        .prepare()
        .expect("application prepares")
        .tooling_document()
        .expect("prepared state projects");
    let plugin = document
        .resources
        .iter()
        .find(|resource| resource.id == "plugin:test/relation-plugin")
        .expect("plugin resource exists");

    assert!(
        plugin
            .facets
            .contains_key("plugin:test/relation-plugin/tooling/routes")
    );
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:test/alternate-facet"
            && resource
                .facets
                .contains_key("plugin:test/alternate-facet/tooling/routes")
    }));
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:test/absent-plugin"
            && resource.labels["decision"] == "relation-endpoint"
    }));
    assert!(
        document
            .resources
            .iter()
            .any(|resource| resource.id == "plugin-slot:test/absent-slot")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin:test/absent-plugin"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Conflicts
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin-slot:test/absent-slot"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contains
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin:test/relation-plugin/tooling/router"
    }));
}
