//! Plugins and protocols the projection tests compose.

use super::*;

#[derive(serde::Deserialize)]
pub(super) struct DuplicateConfig {
    _enabled: bool,
}

impl ConfigProperties for DuplicateConfig {
    const NAME: &'static str = "DuplicateConfig";
}

/// Plugin contributing the first identical config binding.
#[derive(Default)]
pub(super) struct FirstConfigPlugin;

/// Plugin contributing the later identical config binding.
#[derive(Default)]
pub(super) struct SecondConfigPlugin;

impl Plugin for FirstConfigPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/first-config");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.config::<DuplicateConfig>(
            crate::namespaced_id!(crate::ContributionId, "test/first-config"),
            "duplicate",
        );
    }
}

impl Plugin for SecondConfigPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/second-config");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.config::<DuplicateConfig>(
            crate::namespaced_id!(crate::ContributionId, "test/second-config"),
            "duplicate",
        );
    }
}

/// Plugin whose component is displaced by later protocol registration.
#[derive(Default)]
pub(super) struct DisplacedComponentPlugin;

/// Plugin contributing both a component and its trait provider.
#[derive(Default)]
pub(super) struct ProviderContributionPlugin;

/// Early plugin contributing one parser-visible argument provider.
#[cfg(feature = "cli")]
#[derive(Default)]
pub(super) struct CliContributionPlugin;

impl Plugin for DisplacedComponentPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/displaced-component");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/projected-component"),
            COMPONENT,
        );
    }
}

impl Plugin for ProviderContributionPlugin {
    const ID: crate::PluginId =
        crate::namespaced_id!(crate::PluginId, "test/provider-contribution");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/provider-component"),
            COMPONENT,
        );
        contributions.provider(
            crate::namespaced_id!(crate::ContributionId, "test/provider-binding"),
            PROJECTED_PROVIDER_DESCRIPTOR,
        );
    }
}

#[cfg(feature = "cli")]
impl Plugin for CliContributionPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/cli-contribution");

    fn contribute(self, _contributions: &mut PluginContributions) {}

    fn cli(&self, cli: &mut crate::PluginCliRegistrar) {
        cli.args::<ProjectedPluginArgs>(crate::namespaced_id!(
            crate::ContributionId,
            "test/cli-args"
        ));
    }
}

/// Protocol replacing the plugin component with the final manual descriptor.
#[derive(Default)]
pub(super) struct DisplacingProtocol;

impl ProtocolDefinition for DisplacingProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/displacing");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, registry: &mut AppRegistry) {
        registry.components.push(DISPLACED_COMPONENT);
    }

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

pub(super) const PROJECTION_SLOT: PluginSlotId =
    crate::namespaced_id!(PluginSlotId, "test/projection-slot");

/// Optional protocol default used by replacement and suppression projection tests.
#[derive(Default)]
pub(super) struct ProjectionDefaultPlugin;

impl Plugin for ProjectionDefaultPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/projection-default");

    fn contribute(self, _contributions: &mut PluginContributions) {}
}

/// Application-selected replacement used by projection tests.
#[derive(Default)]
pub(super) struct ProjectionReplacementPlugin;

impl Plugin for ProjectionReplacementPlugin {
    const ID: crate::PluginId =
        crate::namespaced_id!(crate::PluginId, "test/projection-replacement");

    fn contribute(self, _contributions: &mut PluginContributions) {}
}

/// Protocol declaring one optional default plugin slot.
#[derive(Default)]
pub(super) struct ProjectionPluginProtocol;

impl ProtocolDefinition for ProjectionPluginProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "test/projection-plugins");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }

    fn register_plugins(plugins: &mut ProtocolPluginRegistrar) {
        plugins.optional_default(PROJECTION_SLOT, ProjectionDefaultPlugin);
    }
}
