//! Attributed plugin contributions: the collector plugins emit into, and how the effective
//! plan lowers their emissions into the application registry.

mod macros;
#[cfg(feature = "tooling")]
mod reconcile;

use std::collections::BTreeSet;

#[cfg(feature = "tooling")]
use std::collections::BTreeMap;

use upwell_config::{ConfigBinding, ConfigProperties};
use upwell_core::{Descriptor, NamespacedIdType};
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

#[cfg(feature = "tooling")]
pub(crate) use reconcile::PluginContributionReconciliation;
#[cfg(feature = "tooling")]
use reconcile::{PluginContributionKey, PluginContributionObservation, reconcile_contributions};

use crate::entry::EntryDescriptor;
use crate::{
    AppRegistry, ContributionId, ContributionProvenance, Contributor, InstallationProvenance,
    PluginId, PluginResolutionPlan,
};

/// The app-neutral category of one attributed plugin contribution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum PluginContributionKind {
    /// A component descriptor contributed to dependency injection.
    Component,
    /// A trait-provider descriptor contributed to dependency injection.
    Provider,
    /// A configuration binding contributed before config resolution.
    ConfigBinding,
    /// An entry point contributed to the application's entry table.
    Entry,
}

/// Public immutable metadata for one effective plugin contribution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PluginContribution {
    provenance: ContributionProvenance,
    kind: PluginContributionKind,
}

impl PluginContribution {
    /// Returns the stable contributor and contributor-local contribution identity.
    pub const fn provenance(self) -> ContributionProvenance {
        self.provenance
    }

    /// Returns the app-neutral contribution category.
    pub const fn kind(self) -> PluginContributionKind {
        self.kind
    }
}

/// The immutable plugin resolution and attributed emissions from its effective plugins.
///
/// Later direct protocol registration may override a lowered descriptor until protocol-owned
/// contributions migrate into this plan. This plan remains the authoritative record of plugin
/// selection and plugin emissions, not yet the final effective application registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectivePluginPlan {
    resolution: PluginResolutionPlan,
    contributions: Vec<PluginContribution>,
    #[cfg(feature = "tooling")]
    observations: Vec<PluginContributionObservation>,
    #[cfg(feature = "tooling")]
    reconciliation: BTreeMap<PluginContributionKey, PluginContributionReconciliation>,
    #[cfg(feature = "tooling")]
    tooling: Vec<crate::tooling::ToolingContributionSet>,
    #[cfg(all(feature = "cli", feature = "tooling"))]
    cli_parser_metadata: Option<upwell_tooling_schema::CliMetadata>,
}

impl EffectivePluginPlan {
    /// Returns the deterministic structural plugin resolution.
    pub const fn resolution(&self) -> &PluginResolutionPlan {
        &self.resolution
    }

    /// Returns plugin emissions in resolution and contributor-local emission order.
    pub const fn emitted_contributions(&self) -> &[PluginContribution] {
        self.contributions.as_slice()
    }

    /// Returns validated owner-scoped generic tooling metadata.
    #[cfg(feature = "tooling")]
    pub(crate) fn tooling_contributions(
        &self,
    ) -> impl Iterator<Item = &crate::tooling::ToolingContributionSet> {
        self.tooling.iter()
    }

    #[cfg(all(feature = "cli", feature = "tooling"))]
    pub(crate) const fn cli_parser_metadata(&self) -> Option<&upwell_tooling_schema::CliMetadata> {
        self.cli_parser_metadata.as_ref()
    }

    #[cfg(feature = "tooling")]
    pub(crate) fn reconcile(&mut self, registry: &AppRegistry) {
        self.reconciliation =
            reconcile_contributions(&self.contributions, &self.observations, registry);
    }

    #[cfg(feature = "tooling")]
    pub(crate) fn selected_contribution(&self, target: &str) -> Option<ContributionProvenance> {
        self.contributions
            .iter()
            .copied()
            .enumerate()
            .find_map(|(index, contribution)| {
                let key = PluginContributionKey::new(contribution.provenance(), index);
                let reconciliation = self
                    .reconciliation
                    .get(&key)
                    .expect("every plugin emission has one reconciliation outcome");

                (reconciliation.decision == PluginContributionDecision::Applied
                    && reconciliation.applied.as_deref() == Some(target))
                .then_some(contribution.provenance())
            })
    }

    #[cfg(feature = "tooling")]
    pub(crate) fn reconciled_contributions(
        &self,
    ) -> impl Iterator<Item = (PluginContribution, &PluginContributionReconciliation)> {
        self.contributions
            .iter()
            .copied()
            .enumerate()
            .map(|(index, contribution)| {
                let key = PluginContributionKey::new(contribution.provenance(), index);
                let reconciliation = self
                    .reconciliation
                    .get(&key)
                    .expect("every plugin emission has one reconciliation outcome");

                (contribution, reconciliation)
            })
    }
}

#[cfg(feature = "tooling")]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// Internal final-registry outcome for one emitted plugin contribution.
pub(crate) enum PluginContributionDecision {
    Applied,
    Displaced,
    NotApplied,
    Duplicate,
}

#[cfg(feature = "tooling")]
impl PluginContributionDecision {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Displaced => "displaced",
            Self::NotApplied => "not-applied",
            Self::Duplicate => "duplicate",
        }
    }
}

/// A provenance-aware collector supplied to one effective plugin.
pub struct PluginContributions {
    contributor: PluginId,
    contributions: Vec<CollectedContribution>,
    #[cfg(feature = "tooling")]
    tooling: crate::ToolingContributions,
}

impl PluginContributions {
    pub(super) fn new(contributor: PluginId) -> Self {
        Self {
            contributor,
            contributions: Vec::new(),
            #[cfg(feature = "tooling")]
            tooling: crate::ToolingContributions::new(format!("plugin:{}", contributor.as_str())),
        }
    }

    /// Contributes component type `T` through its static descriptor.
    pub fn component<T>(&mut self, id: ContributionId)
    where
        T: Descriptor<ComponentDescriptor>,
    {
        self.component_descriptor(id, <T as Descriptor<ComponentDescriptor>>::DESCRIPTOR);
    }

    /// Contributes a raw component descriptor when no typed descriptor implementation exists.
    pub fn component_descriptor(&mut self, id: ContributionId, descriptor: ComponentDescriptor) {
        self.push(id, ContributionPayload::Component(descriptor));
    }

    /// Contributes a trait-provider descriptor.
    pub fn provider(&mut self, id: ContributionId, descriptor: ProviderDescriptor) {
        self.push(id, ContributionPayload::Provider(descriptor));
    }

    /// Contributes a configuration binding.
    pub fn config<T: ConfigProperties>(&mut self, id: ContributionId, path: impl Into<String>) {
        self.push(
            id,
            ContributionPayload::ConfigBinding(ConfigBinding::of::<T>(path)),
        );
    }

    /// Contributes an entry point.
    pub fn entry(&mut self, id: ContributionId, descriptor: EntryDescriptor) {
        self.push(id, ContributionPayload::Entry(descriptor));
    }

    /// Returns this plugin's owner-scoped generic tooling metadata collector.
    #[cfg(feature = "tooling")]
    pub fn tooling(&mut self) -> &mut crate::ToolingContributions {
        &mut self.tooling
    }

    fn push(&mut self, id: ContributionId, payload: ContributionPayload) {
        let provenance = ContributionProvenance::new(Contributor::Plugin(self.contributor), id);
        let metadata = PluginContribution {
            provenance,
            kind: payload.kind(),
        };

        self.contributions
            .push(CollectedContribution { metadata, payload });
    }

    pub(super) fn finish(self) -> Result<CollectedPluginContributions, PluginPlanError> {
        let mut identities = BTreeSet::new();

        for contribution in &self.contributions {
            let provenance = contribution.metadata.provenance;

            if provenance
                .contribution()
                .is_in_namespace(upwell_core::FRAMEWORK_NAMESPACE)
            {
                return Err(PluginPlanError::ReservedContributionNamespace { provenance });
            }

            if !identities.insert(provenance.contribution()) {
                return Err(PluginPlanError::DuplicateContribution { provenance });
            }
        }

        Ok(CollectedPluginContributions {
            contributions: self.contributions,
            #[cfg(feature = "tooling")]
            tooling: self.tooling.finish()?,
        })
    }
}

/// A typed failure while freezing or lowering retained plugin state.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginPlanError {
    /// One plugin declared the same contributor-local CLI provider identity more than once.
    #[cfg(feature = "cli")]
    #[error("plugin CLI provider '{provenance:?}' is declared more than once")]
    DuplicateCliProvider {
        /// The duplicated stable CLI provider provenance.
        provenance: ContributionProvenance,
    },

    /// A third-party plugin attempted to claim a framework-owned CLI provider identity.
    #[cfg(feature = "cli")]
    #[error("plugin CLI provider '{provenance:?}' uses the reserved 'upwell/' namespace")]
    ReservedCliProviderNamespace {
        /// The invalid stable CLI provider provenance.
        provenance: ContributionProvenance,
    },

    /// One plugin emitted the same contributor-local identity more than once.
    #[error("plugin contribution '{provenance:?}' is declared more than once")]
    DuplicateContribution {
        /// The duplicated stable contribution provenance.
        provenance: ContributionProvenance,
    },

    /// A third-party plugin attempted to claim a framework-owned contribution identity.
    #[error("plugin contribution '{provenance:?}' uses the reserved 'upwell/' namespace")]
    ReservedContributionNamespace {
        /// The invalid contribution provenance.
        provenance: ContributionProvenance,
    },

    /// Owner-scoped generic tooling metadata is structurally invalid.
    #[cfg(feature = "tooling")]
    #[error(transparent)]
    Tooling(#[from] crate::ToolingContributionError),

    /// The structural resolver selected a plugin whose retained payload is absent.
    #[error("resolved plugin '{plugin}' has no retained installation ({provenance:?})")]
    MissingInstallation {
        /// The selected stable plugin identity.
        plugin: PluginId,
        /// The selected installation provenance.
        provenance: InstallationProvenance,
    },
}

pub(crate) struct CollectedPluginPlan {
    pub(super) resolution: PluginResolutionPlan,
    pub(super) contributions: Vec<CollectedContribution>,
    #[cfg(feature = "tooling")]
    pub(super) tooling: Vec<crate::tooling::ToolingContributionSet>,
    #[cfg(all(feature = "cli", feature = "tooling"))]
    pub(super) cli_parser_metadata: Option<upwell_tooling_schema::CliMetadata>,
}

impl CollectedPluginPlan {
    pub(crate) fn lower(self, registry: &mut AppRegistry) -> EffectivePluginPlan {
        let mut metadata = Vec::with_capacity(self.contributions.len());
        #[cfg(feature = "tooling")]
        let mut observations = Vec::with_capacity(self.contributions.len());

        for contribution in self.contributions {
            #[cfg(feature = "tooling")]
            observations.push(PluginContributionObservation::capture(
                &contribution.payload,
                registry,
            ));
            metadata.push(contribution.metadata);

            match contribution.payload {
                ContributionPayload::Component(descriptor) => {
                    registry.components.push(descriptor);
                }
                ContributionPayload::Provider(descriptor) => {
                    registry.providers.push(descriptor);
                }
                ContributionPayload::ConfigBinding(binding) => {
                    registry.config_bindings.push(binding);
                }
                ContributionPayload::Entry(descriptor) => {
                    registry.entries.push(descriptor);
                }
            }
        }

        EffectivePluginPlan {
            resolution: self.resolution,
            contributions: metadata,
            #[cfg(feature = "tooling")]
            observations,
            #[cfg(feature = "tooling")]
            reconciliation: BTreeMap::new(),
            #[cfg(feature = "tooling")]
            tooling: self.tooling,
            #[cfg(all(feature = "cli", feature = "tooling"))]
            cli_parser_metadata: self.cli_parser_metadata,
        }
    }
}

pub(super) struct CollectedContribution {
    metadata: PluginContribution,
    payload: ContributionPayload,
}

/// Collected runtime and tooling emissions from one synchronously consumed plugin.
pub(super) struct CollectedPluginContributions {
    pub(super) contributions: Vec<CollectedContribution>,
    #[cfg(feature = "tooling")]
    pub(super) tooling: crate::tooling::ToolingContributionSet,
}

#[cfg(test)]
impl CollectedPluginContributions {
    pub(super) fn len(&self) -> usize {
        self.contributions.len()
    }
}

impl CollectedContribution {
    #[cfg(feature = "cli")]
    pub(super) const fn provenance(&self) -> ContributionProvenance {
        self.metadata.provenance
    }
}

enum ContributionPayload {
    Component(ComponentDescriptor),
    Provider(ProviderDescriptor),
    ConfigBinding(ConfigBinding),
    Entry(EntryDescriptor),
}

impl ContributionPayload {
    const fn kind(&self) -> PluginContributionKind {
        match self {
            Self::Component(_) => PluginContributionKind::Component,
            Self::Provider(_) => PluginContributionKind::Provider,
            Self::ConfigBinding(_) => PluginContributionKind::ConfigBinding,
            Self::Entry(_) => PluginContributionKind::Entry,
        }
    }
}
