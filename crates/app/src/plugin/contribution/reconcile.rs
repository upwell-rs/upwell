//! Tooling-only reconciliation of plugin emissions against the final application registry.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};

use upwell_config::ConfigBinding;
use upwell_core::EntryKey;
use upwell_di::ProviderDescriptor;

use super::{ContributionPayload, PluginContribution, PluginContributionDecision};
use crate::{AppRegistry, ContributionProvenance};

impl ContributionPayload {
    fn target(&self) -> ContributionTarget {
        match self {
            Self::Component(descriptor) => ContributionTarget::new(
                format!("component:{}", descriptor.id),
                ContributionTargetKey::Component(descriptor.ty.type_id),
            ),
            Self::Provider(descriptor) => ContributionTarget::new(
                provider_target(descriptor),
                ContributionTargetKey::Provider {
                    trait_ty: descriptor.trait_ty.type_id,
                    concrete_ty: descriptor.concrete_ty.type_id,
                    qualifier: descriptor.qualifier.to_string(),
                },
            ),
            Self::ConfigBinding(binding) => ContributionTarget::new(
                config_binding_target(binding),
                ContributionTargetKey::ConfigBinding {
                    ty: binding.ty.type_id,
                    path: binding.path.clone(),
                },
            ),
            Self::Entry(descriptor) => ContributionTarget::new(
                entry_target(descriptor.key),
                ContributionTargetKey::Entry(descriptor.key),
            ),
        }
    }

    fn exists_in(&self, registry: &AppRegistry) -> bool {
        match self {
            Self::Component(descriptor) => registry.components.iter().any(|existing| {
                existing.ty.type_id == descriptor.ty.type_id && existing.id == descriptor.id
            }),
            Self::Provider(descriptor) => registry.providers.iter().any(|existing| {
                existing.trait_ty.type_id == descriptor.trait_ty.type_id
                    && existing.concrete_ty.type_id == descriptor.concrete_ty.type_id
                    && existing.qualifier == descriptor.qualifier
            }),
            Self::ConfigBinding(binding) => registry.config_bindings.iter().any(|existing| {
                existing.ty.type_id == binding.ty.type_id && existing.path == binding.path
            }),
            Self::Entry(descriptor) => registry
                .entries
                .iter()
                .any(|existing| existing.key == descriptor.key),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Pre-lowering target observation retained only to interpret the final registry.
pub(super) struct PluginContributionObservation {
    requested: String,
    key: ContributionTargetKey,
    preceded: bool,
}

impl PluginContributionObservation {
    pub(super) fn capture(payload: &ContributionPayload, registry: &AppRegistry) -> Self {
        let target = payload.target();

        Self {
            requested: target.requested,
            key: target.key,
            preceded: payload.exists_in(registry),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Final-registry reconciliation for one provenance-and-index keyed plugin emission.
pub(crate) struct PluginContributionReconciliation {
    pub(crate) requested: String,
    pub(crate) applied: Option<String>,
    pub(crate) decision: PluginContributionDecision,
}

impl PluginContributionObservation {
    fn final_target(&self, registry: &AppRegistry) -> Option<String> {
        match &self.key {
            ContributionTargetKey::Component(ty) => registry
                .components
                .iter()
                .find(|component| component.ty.type_id == *ty)
                .map(|component| format!("component:{}", component.id)),
            ContributionTargetKey::Provider {
                trait_ty,
                concrete_ty,
                qualifier,
            } => registry
                .providers
                .iter()
                .find(|provider| {
                    provider.trait_ty.type_id == *trait_ty
                        && provider.concrete_ty.type_id == *concrete_ty
                        && provider.qualifier == qualifier
                })
                .map(provider_target),
            ContributionTargetKey::ConfigBinding { ty, path } => registry
                .config_bindings
                .iter()
                .find(|binding| binding.ty.type_id == *ty && binding.path == *path)
                .map(config_binding_target),
            ContributionTargetKey::Entry(key) => registry
                .entries
                .iter()
                .find(|entry| entry.key == *key)
                .map(|entry| entry_target(entry.key)),
        }
    }
}

pub(super) fn reconcile_contributions(
    contributions: &[PluginContribution],
    observations: &[PluginContributionObservation],
    registry: &AppRegistry,
) -> BTreeMap<PluginContributionKey, PluginContributionReconciliation> {
    let mut selected = BTreeSet::new();
    let mut reconciliations = BTreeMap::new();

    for (index, (contribution, observation)) in
        contributions.iter().copied().zip(observations).enumerate()
    {
        let key = PluginContributionKey::new(contribution.provenance(), index);
        let final_target = observation.final_target(registry);
        let selected_key = final_target
            .as_ref()
            .map(|applied| (observation.key.clone(), applied.clone()));

        let decision = match (&final_target, selected_key) {
            (None, _) => PluginContributionDecision::NotApplied,
            (Some(applied), _) if observation.preceded && applied == &observation.requested => {
                PluginContributionDecision::Duplicate
            }
            (Some(_), Some(key)) if !selected.insert(key.clone()) => {
                PluginContributionDecision::Duplicate
            }
            (Some(applied), _) if applied == &observation.requested => {
                PluginContributionDecision::Applied
            }
            (Some(_), _) => PluginContributionDecision::Displaced,
        };

        reconciliations.insert(
            key,
            PluginContributionReconciliation {
                requested: observation.requested.clone(),
                applied: final_target,
                decision,
            },
        );
    }

    reconciliations
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
/// Stable private key for one contributor-local emission occurrence.
pub(super) struct PluginContributionKey {
    provenance: ContributionProvenance,
    index: usize,
}

impl PluginContributionKey {
    pub(super) const fn new(provenance: ContributionProvenance, index: usize) -> Self {
        Self { provenance, index }
    }
}

/// Tooling-only requested resource identity and lookup key.
struct ContributionTarget {
    requested: String,
    key: ContributionTargetKey,
}

impl ContributionTarget {
    fn new(requested: String, key: ContributionTargetKey) -> Self {
        Self { requested, key }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
/// Stable registry lookup key for an emitted resource category.
enum ContributionTargetKey {
    Component(TypeId),
    Provider {
        trait_ty: TypeId,
        concrete_ty: TypeId,
        qualifier: String,
    },
    ConfigBinding {
        ty: TypeId,
        path: String,
    },
    Entry(EntryKey),
}

fn provider_target(descriptor: &ProviderDescriptor) -> String {
    format!(
        "provider:{}:{}:{}",
        (descriptor.trait_ty.type_name)(),
        (descriptor.concrete_ty.type_name)(),
        descriptor.qualifier
    )
}

fn config_binding_target(binding: &ConfigBinding) -> String {
    format!(
        "config-binding:{}:{}",
        (binding.ty.type_name)(),
        binding.path
    )
}

fn entry_target(key: EntryKey) -> String {
    format!("entry:{key}")
}
