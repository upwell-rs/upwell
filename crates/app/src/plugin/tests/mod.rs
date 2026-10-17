use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_config::{ConfigManager, ConfigProperties, Toml};
use upwell_core::TypeDescriptor;
use upwell_di::{Component, ComponentDescriptor, ProviderDescriptor};

use super::{
    Plugin, PluginCatalog, PluginContributions, PluginWithOptions, ProtocolPluginRegistrar,
};
use crate::composition::{PluginRelation, RelationKind, RelationTarget};
use crate::test_support::{planned_provider, test_component, unbuilt_factories};
use crate::{
    App, AppBuilder, AppHost, AppRegistry, ExecutionMode, ProtocolDefinition, ScopeTopology,
};

static PROTOCOL_DISCOVERIES: AtomicUsize = AtomicUsize::new(0);
static PROTOCOL_CONTRIBUTIONS: AtomicUsize = AtomicUsize::new(0);
static APPLICATION_DISCOVERIES: AtomicUsize = AtomicUsize::new(0);
static APPLICATION_CONTRIBUTIONS: AtomicUsize = AtomicUsize::new(0);
static STATEFUL_VALUES: AtomicUsize = AtomicUsize::new(0);
static DEFAULT_DISCOVERIES: AtomicUsize = AtomicUsize::new(0);
static DEFAULT_CONTRIBUTIONS: AtomicUsize = AtomicUsize::new(0);
static REPLACEMENT_DISCOVERIES: AtomicUsize = AtomicUsize::new(0);
static REPLACEMENT_CONTRIBUTIONS: AtomicUsize = AtomicUsize::new(0);
static CONSTRUCTED_VALUES: AtomicUsize = AtomicUsize::new(0);

const TEST_SLOT: crate::PluginSlotId =
    crate::namespaced_id!(crate::PluginSlotId, "test/default-slot");

struct ProtocolPluginComponent;

struct ApplicationPluginComponent;

test_component!(ProtocolPluginComponent, "protocol_plugin_component");
test_component!(ApplicationPluginComponent, "application_plugin_component");

#[derive(serde::Deserialize)]
struct PluginConfig {
    pub _enabled: bool,
}

impl ConfigProperties for PluginConfig {
    const NAME: &'static str = "PluginConfig";
}

static PLUGIN_PROVIDER: ProviderDescriptor = planned_provider(
    TypeDescriptor::of::<ProtocolPluginComponent>("PluginTrait"),
    TypeDescriptor::of::<ApplicationPluginComponent>(ApplicationPluginComponent::NAME),
    "application_plugin_component",
    true,
);

static PROTOCOL_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ProtocolPluginComponent>()
        .with_factories(unbuilt_factories!("static", Vec::new, true))
        .without_retention();

static APPLICATION_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ApplicationPluginComponent>()
        .with_factories(unbuilt_factories!("static", Vec::new, true))
        .without_retention();

impl upwell_core::Descriptor<ComponentDescriptor> for ApplicationPluginComponent {
    const DESCRIPTOR: ComponentDescriptor = APPLICATION_COMPONENT;
}

struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/protocol-plugin");

    fn contribute(self, contributions: &mut PluginContributions) {
        PROTOCOL_CONTRIBUTIONS.fetch_add(1, Ordering::SeqCst);
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/protocol-component"),
            PROTOCOL_COMPONENT,
        );
    }

    fn auto_discover(&mut self) {
        PROTOCOL_DISCOVERIES.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ApplicationPlugin;

impl Plugin for ApplicationPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/application-plugin");
    const RELATIONS: &'static [PluginRelation] = &[PluginRelation::new(
        RelationKind::Requires,
        RelationTarget::Plugin(ProtocolPlugin::ID),
    )];

    fn contribute(self, contributions: &mut PluginContributions) {
        APPLICATION_CONTRIBUTIONS.fetch_add(1, Ordering::SeqCst);
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/application-component"),
            APPLICATION_COMPONENT,
        );
    }

    fn auto_discover(&mut self) {
        APPLICATION_DISCOVERIES.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct DuplicateContributionPlugin;

impl Plugin for DuplicateContributionPlugin {
    const ID: crate::PluginId =
        crate::namespaced_id!(crate::PluginId, "test/duplicate-contribution");

    fn contribute(self, contributions: &mut PluginContributions) {
        const ID: crate::ContributionId =
            crate::namespaced_id!(crate::ContributionId, "test/duplicate");

        contributions.component_descriptor(ID, PROTOCOL_COMPONENT);
        contributions.component_descriptor(ID, APPLICATION_COMPONENT);
    }
}

struct StatefulPlugin {
    value: usize,
}

#[derive(Default)]
struct AllContributionKindsPlugin;

impl Plugin for AllContributionKindsPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/all-kinds");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/all-kinds-component"),
            APPLICATION_COMPONENT,
        );
        contributions.provider(
            crate::namespaced_id!(crate::ContributionId, "test/all-kinds-provider"),
            PLUGIN_PROVIDER,
        );
        contributions.config::<PluginConfig>(
            crate::namespaced_id!(crate::ContributionId, "test/all-kinds-config"),
            "plugin",
        );
    }
}

#[derive(Default)]
struct OrphanProviderPlugin;

impl Plugin for OrphanProviderPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/orphan-provider");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.provider(
            crate::namespaced_id!(crate::ContributionId, "test/orphan-provider"),
            PLUGIN_PROVIDER,
        );
    }
}

#[derive(Default)]
struct FirstDuplicatePayloadPlugin;

impl Plugin for FirstDuplicatePayloadPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/first-payload");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/first-payload"),
            APPLICATION_COMPONENT,
        );
    }
}

#[derive(Default)]
struct SecondDuplicatePayloadPlugin;

impl Plugin for SecondDuplicatePayloadPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/second-payload");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/second-payload"),
            APPLICATION_COMPONENT,
        );
    }
}

#[derive(Default)]
struct DefaultPlugin;

impl Plugin for DefaultPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/default-plugin");

    fn contribute(self, _contributions: &mut PluginContributions) {
        DEFAULT_CONTRIBUTIONS.fetch_add(1, Ordering::SeqCst);
    }

    fn auto_discover(&mut self) {
        DEFAULT_DISCOVERIES.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ReplacementPlugin;

impl Plugin for ReplacementPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/replacement-plugin");

    fn contribute(self, _contributions: &mut PluginContributions) {
        REPLACEMENT_CONTRIBUTIONS.fetch_add(1, Ordering::SeqCst);
    }

    fn auto_discover(&mut self) {
        REPLACEMENT_DISCOVERIES.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct DefaultProtocol;

impl ProtocolDefinition for DefaultProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/default-protocol");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(
        self,
        _context: &crate::ValidationContext<'_>,
    ) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }

    fn register_plugins(plugins: &mut ProtocolPluginRegistrar) {
        plugins.optional_default(TEST_SLOT, DefaultPlugin);
    }
}

struct ReplacementHost;

impl AppHost for ReplacementHost {
    type Protocol = DefaultProtocol;

    fn builder() -> Result<AppBuilder<Self::Protocol>, upwell_config::ConfigError> {
        Ok(
            App::<DefaultProtocol>::builder("static-plugin-declarations")
                .config_source(ConfigManager::<Toml>::empty())
                .auto_discover(),
        )
    }

    fn declare_plugins(plugins: &mut super::ApplicationPluginRegistrar) {
        plugins.replace(TEST_SLOT, ReplacementPlugin);
    }
}

impl Default for StatefulPlugin {
    fn default() -> Self {
        Self { value: 41 }
    }
}

impl Plugin for StatefulPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/stateful");

    fn contribute(self, _contributions: &mut PluginContributions) {
        STATEFUL_VALUES.fetch_add(self.value, Ordering::SeqCst);
    }

    fn auto_discover(&mut self) {
        self.value += 1;
    }
}

/// A non-default plugin constructed through explicit options or a supplied instance.
struct ConstructedPlugin {
    value: usize,
}

impl Plugin for ConstructedPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/constructed");

    fn contribute(self, _contributions: &mut PluginContributions) {
        CONSTRUCTED_VALUES.fetch_add(self.value, Ordering::SeqCst);
    }
}

impl PluginWithOptions for ConstructedPlugin {
    type Options = usize;

    fn from_options(options: Self::Options) -> Self {
        Self { value: options }
    }
}

#[derive(Default)]
struct TestProtocol;

impl ProtocolDefinition for TestProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/plugin-plan");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(
        self,
        context: &crate::ValidationContext<'_>,
    ) -> Result<Self::Prepared, Self::Error> {
        assert_eq!(context.plugin_plan().resolution().plugins().len(), 2);

        Ok(())
    }

    fn register_plugins(plugins: &mut ProtocolPluginRegistrar) {
        plugins.mandatory(ProtocolPlugin);
    }
}

#[derive(Default)]
struct OrphanProviderProtocol;

impl ProtocolDefinition for OrphanProviderProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/orphan-provider");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, registry: &mut AppRegistry) {
        registry.providers.push(PLUGIN_PROVIDER);
    }

    fn prepare(
        self,
        _context: &crate::ValidationContext<'_>,
    ) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

mod contract;
mod lowering;
mod slots;
