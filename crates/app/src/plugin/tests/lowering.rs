//! Plugin discovery and contribution lowering into the app registry.

use super::*;

#[test]
fn protocol_and_application_plugins_lower_once_in_resolved_order() {
    PROTOCOL_DISCOVERIES.store(0, Ordering::SeqCst);
    PROTOCOL_CONTRIBUTIONS.store(0, Ordering::SeqCst);
    APPLICATION_DISCOVERIES.store(0, Ordering::SeqCst);
    APPLICATION_CONTRIBUTIONS.store(0, Ordering::SeqCst);

    let prepared = App::<TestProtocol>::builder("plugin-plan")
        .auto_discover()
        .register_plugin::<ApplicationPlugin>()
        .prepare()
        .expect("plugin plan prepares");
    let plugins = prepared.plugin_plan().resolution().plugins();
    let components = &prepared.registry().components;

    assert_eq!(plugins[0].id(), ProtocolPlugin::ID);
    assert_eq!(plugins[1].id(), ApplicationPlugin::ID);
    let protocol_index = components
        .iter()
        .position(|component| component.id == ProtocolPluginComponent::ID)
        .expect("protocol plugin component is lowered");
    let application_index = components
        .iter()
        .position(|component| component.id == ApplicationPluginComponent::ID)
        .expect("application plugin component is lowered");

    assert!(protocol_index < application_index);
    assert_eq!(PROTOCOL_DISCOVERIES.load(Ordering::SeqCst), 1);
    assert_eq!(PROTOCOL_CONTRIBUTIONS.load(Ordering::SeqCst), 1);
    assert_eq!(APPLICATION_DISCOVERIES.load(Ordering::SeqCst), 1);
    assert_eq!(APPLICATION_CONTRIBUTIONS.load(Ordering::SeqCst), 1);
}

#[test]
fn duplicate_contribution_ids_are_typed_errors() {
    let result = App::<()>::builder("duplicate-contribution")
        .register_plugin::<DuplicateContributionPlugin>()
        .prepare();

    let Err(error) = result else {
        panic!("duplicate contribution must fail");
    };

    assert!(matches!(
        error,
        crate::Error::PluginPlan(crate::plugin::PluginPlanError::DuplicateContribution { .. })
    ));
}

#[test]
fn plugin_provider_without_component_is_rejected_during_preparation() {
    let result = App::<()>::builder("orphan-plugin-provider")
        .register_plugin::<OrphanProviderPlugin>()
        .prepare();
    let Err(error) = result else {
        panic!("orphan plugin provider must fail preparation");
    };

    assert!(matches!(
        error,
        crate::Error::Di(upwell_di::Error::ProviderComponentMissing(_))
    ));
}

#[test]
fn protocol_provider_without_component_is_rejected_during_preparation() {
    let result = App::<OrphanProviderProtocol>::builder("orphan-protocol-provider").prepare();
    let Err(error) = result else {
        panic!("orphan protocol provider must fail preparation");
    };

    assert!(matches!(
        error,
        crate::Error::Di(upwell_di::Error::ProviderComponentMissing(_))
    ));
}

#[test]
fn independently_prepared_apps_own_distinct_retained_plugin_state() {
    STATEFUL_VALUES.store(0, Ordering::SeqCst);

    App::<()>::builder("first-stateful-app")
        .auto_discover()
        .register_plugin::<StatefulPlugin>()
        .prepare()
        .expect("first stateful plugin prepares");
    App::<()>::builder("second-stateful-app")
        .auto_discover()
        .register_plugin::<StatefulPlugin>()
        .prepare()
        .expect("second stateful plugin prepares");

    assert_eq!(STATEFUL_VALUES.load(Ordering::SeqCst), 84);
}

#[test]
fn options_and_supplied_instances_converge_on_retained_plugins() {
    CONSTRUCTED_VALUES.store(0, Ordering::SeqCst);

    App::<()>::builder("options-plugin")
        .register_plugin_with_options::<ConstructedPlugin>(20)
        .prepare()
        .expect("options plugin prepares");
    App::<()>::builder("instance-plugin")
        .with_plugin(ConstructedPlugin { value: 22 })
        .prepare()
        .expect("supplied plugin prepares");

    assert_eq!(CONSTRUCTED_VALUES.load(Ordering::SeqCst), 42);
}

#[test]
fn contribution_macro_supports_typed_and_expression_component_paths() {
    let mut contributions = PluginContributions::new(AllContributionKindsPlugin::ID);

    crate::contribute! {
        to &mut contributions,
        components: [
            "test/macro-typed" => type ApplicationPluginComponent,
            "test/macro-expression" => PROTOCOL_COMPONENT
        ],
        providers: [
            "test/macro-provider" => PLUGIN_PROVIDER,
        ],
        configs: [
            "test/macro-config" => PluginConfig => "plugin",
        ],
    }

    let contributions = contributions.finish().expect("macro IDs are unique");

    assert_eq!(contributions.len(), 4);
}

#[test]
fn discovery_is_skipped_when_the_builder_does_not_enable_it() {
    STATEFUL_VALUES.store(0, Ordering::SeqCst);

    App::<()>::builder("undiscovered-stateful-app")
        .register_plugin::<StatefulPlugin>()
        .prepare()
        .expect("stateful plugin prepares without discovery");

    assert_eq!(STATEFUL_VALUES.load(Ordering::SeqCst), 41);
}

#[test]
fn every_app_neutral_contribution_kind_lowers_with_metadata() {
    let mut catalog = PluginCatalog::new();
    let mut registry = AppRegistry::default();

    catalog.with_plugin(AllContributionKindsPlugin);

    let plan = catalog
        .freeze::<()>(false)
        .expect("all contribution kinds freeze")
        .lower(&mut registry);
    let kinds: Vec<_> = plan
        .emitted_contributions()
        .iter()
        .map(|contribution| contribution.kind())
        .collect();

    assert_eq!(registry.components.len(), 1);
    assert_eq!(registry.providers.len(), 1);
    assert_eq!(registry.config_bindings.len(), 1);
    assert_eq!(
        kinds,
        [
            crate::plugin::PluginContributionKind::Component,
            crate::plugin::PluginContributionKind::Provider,
            crate::plugin::PluginContributionKind::ConfigBinding,
        ]
    );
}

#[test]
fn duplicate_payloads_are_all_lowered_before_registry_resolution() {
    let mut catalog = PluginCatalog::new();
    let mut registry = AppRegistry::default();

    catalog.with_plugin(FirstDuplicatePayloadPlugin);
    catalog.with_plugin(SecondDuplicatePayloadPlugin);

    let plan = catalog
        .freeze::<()>(false)
        .expect("duplicate payload plugins freeze")
        .lower(&mut registry);

    assert_eq!(plan.emitted_contributions().len(), 2);
    assert_eq!(registry.components.len(), 2);
    assert_eq!(registry.components[0].id, ApplicationPluginComponent::ID);
    assert_eq!(registry.components[1].id, ApplicationPluginComponent::ID);
    assert_eq!(
        registry
            .resolved_components()
            .expect("registry resolves duplicate payloads")
            .len(),
        1
    );
}

#[test]
fn every_app_neutral_contribution_kind_participates_in_app_preparation() {
    let prepared = App::<()>::builder("all-plugin-contribution-kinds")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [plugin]
                    _enabled = true
                "#,
            )
            .expect("plugin config parses"),
        )
        .register_plugin::<AllContributionKindsPlugin>()
        .prepare()
        .expect("all plugin contribution kinds validate");

    assert_eq!(prepared.registry().providers.len(), 1);
    assert_eq!(prepared.registry().config_bindings.len(), 1);
    assert_eq!(prepared.plugin_plan().emitted_contributions().len(), 3);
    assert_eq!(*prepared.protocol(), ());
}

#[tokio::test]
async fn built_apps_retain_the_prepared_plugin_record() {
    let prepared = App::<()>::builder("built-plugin-record")
        .register_plugin::<StatefulPlugin>()
        .prepare()
        .expect("plugin record prepares");
    let expected = prepared.plugin_plan().clone();
    let app = prepared.build().await.expect("application builds");

    assert_eq!(app.plugin_plan(), &expected);
}
