//! Config binding ownership, lifecycle and dependency projection.

use super::*;

#[test]
fn duplicate_plugin_config_bindings_select_one_owner() {
    let document = App::<()>::builder("duplicate-plugin-config")
        .config_source(
            ConfigManager::<Toml>::from_str("[duplicate]\n_enabled = true")
                .expect("duplicate config fixture parses"),
        )
        .register_plugin::<FirstConfigPlugin>()
        .register_plugin::<SecondConfigPlugin>()
        .prepare()
        .expect("duplicate config contributions prepare")
        .tooling_document()
        .expect("duplicate config contributions project");
    let selected_config = document
        .resources
        .iter()
        .find(|resource| {
            resource.id.ends_with(":duplicate")
                && resource.kind == upwell_tooling_schema::ResourceKind::ConfigBinding
        })
        .expect("selected config binding exists");
    let mut decisions: Vec<_> = document
        .resources
        .iter()
        .filter(|resource| {
            resource.id.starts_with("contribution:plugin:test/")
                && resource.labels.get("contribution-kind") == Some(&String::from("config-binding"))
        })
        .map(|resource| resource.labels["decision"].as_str())
        .collect();

    decisions.sort_unstable();

    assert_eq!(decisions, ["applied", "duplicate"]);
    assert_eq!(
        selected_config
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/first-config")
    );
}

#[test]
fn lifecycle_projection_classifies_phases_and_links_config_reload_hooks() {
    let mut prepared = App::<()>::builder("lifecycle-projection")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&RELOAD_HOOK_COMPONENT)
        .prepare()
        .expect("lifecycle fixture prepares");

    prepared.retain_host_lifecycle(crate::app::HostLifecycleCapabilities::new(
        true, false, true, false, true,
    ));

    let document = prepared
        .tooling_document()
        .expect("lifecycle fixture projects");
    let prepare = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:prepare")
        .expect("prepare lifecycle exists");
    let reload = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:config_reload")
        .expect("config reload lifecycle exists");
    let setup = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:setup")
        .expect("setup host phase exists");

    assert_eq!(prepare.labels["category"], "construction-phase");
    assert_eq!(reload.labels["category"], "runtime-event");
    assert_eq!(reload.labels["repeatable"], "true");
    assert_eq!(setup.labels["category"], "host-phase");
    assert_eq!(setup.labels["callback"], "true");
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Hooks && relationship.to == "lifecycle:config_reload"
    }));
    assert!(!document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "lifecycle:startup"
            && relationship.to == "lifecycle:shutdown"
    }));
}

#[test]
fn provider_ordering_connects_matching_provider_resources() {
    let document = App::<()>::builder("provider-ordering")
        .config_source(ConfigManager::<Toml>::empty())
        .with_component(FirstProvider)
        .with_component(SecondProvider)
        .provider_descriptor(&FIRST_PROVIDER_DESCRIPTOR)
        .provider_descriptor(&SECOND_PROVIDER_DESCRIPTOR)
        .prepare()
        .expect("provider ordering prepares")
        .tooling_document()
        .expect("provider ordering projects");
    let ordering = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::OrdersBefore
                && relationship.from.starts_with("provider:")
                && relationship.to.starts_with("provider:")
        })
        .expect("provider-resource ordering exists");

    assert!(ordering.from.contains("FirstProvider"));
    assert!(ordering.to.contains("SecondProvider"));
    assert!(ordering.labels["rust-trait"].contains("OrderedProvider"));
    assert!(!document.relationships.iter().any(|relationship| {
        matches!(
            relationship.kind,
            RelationshipKind::OrdersBefore | RelationshipKind::OrdersAfter
        ) && relationship.from.starts_with("component:")
            && relationship.to.starts_with("component:")
            && relationship.labels.contains_key("rust-trait")
    }));
}

#[test]
fn config_dependencies_target_exact_bindings_or_mark_type_ambiguity() {
    let document = App::<()>::builder("config-dependencies")
        .config_source(
            ConfigManager::<Toml>::from_str(
                "[duplicate]\n_enabled = true\n[alternate]\n_enabled = false",
            )
            .expect("config dependency fixture parses"),
        )
        .config::<DuplicateConfig>("duplicate")
        .config::<DuplicateConfig>("alternate")
        .component_descriptor(&CONFIG_CONSUMER_COMPONENT)
        .component_descriptor(&AMBIGUOUS_CONFIG_HOOK_COMPONENT)
        .with_component(ReloadHookComponent)
        .prepare()
        .expect("config dependency fixture prepares")
        .tooling_document()
        .expect("config dependency fixture projects");
    let exact = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship.from == "component:config-consumer"
                && relationship.to.starts_with("config-binding:")
        })
        .expect("exact component config dependency exists");
    let ambiguous = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship
                    .from
                    .starts_with("hook:reload-hook-component:config_reload:")
        })
        .expect("ambiguous hook config dependency exists");

    assert!(exact.to.starts_with("config-binding:"));
    assert!(exact.to.ends_with(":duplicate"));
    assert!(ambiguous.to.starts_with("type:"));
    assert_eq!(ambiguous.labels["binding-resolution"], "ambiguous");
    assert_eq!(ambiguous.labels["binding-cardinality"], "2");
}

#[test]
fn config_dependencies_distinguish_missing_bindings() {
    let document = App::<()>::builder("missing-config-dependency")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&MISSING_CONFIG_HOOK_COMPONENT)
        .with_component(ReloadHookComponent)
        .prepare()
        .expect("missing hook config dependency prepares")
        .tooling_document()
        .expect("missing hook config dependency projects");
    let missing = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship
                    .from
                    .starts_with("hook:reload-hook-component:config_reload:")
        })
        .expect("missing hook config dependency exists");

    assert_eq!(missing.labels["binding-resolution"], "missing");
    assert_eq!(missing.labels["binding-cardinality"], "0");
}
