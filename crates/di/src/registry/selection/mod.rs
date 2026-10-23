use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{ResolutionMode, Scope, ScopeId};

use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};

mod dependency;
pub(crate) mod rules;
mod runtime;

pub(crate) use rules::{provider_matches, select, select_single_by, validate_provider_components};

/// Producer-authoritative target selected for one validated dependency edge.
#[derive(Clone, Copy, Debug)]
pub enum DependencyTarget {
    /// A dependency resolved directly by its concrete component type.
    Component(ComponentDescriptor),
    /// A trait dependency resolved through one provider descriptor.
    Provider(ProviderDescriptor),
}

/// Stable explanation of why a validated dependency target is selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencySelectionReason {
    /// The requested type is an effective concrete component.
    DirectConcrete,
    /// A qualifier selected this provider using runtime qualifier precedence.
    Qualified,
    /// The selected runtime provider set contains exactly one provider.
    SoleProviderInWinningSet,
    /// The selected runtime provider set contains one unique primary provider.
    PrimaryProviderInWinningSet,
    /// A collection includes every runtime-visible provider in provider order.
    Collection,
    /// A keyed dependency includes the runtime winner for one qualifier.
    Keyed,
}

impl DependencySelectionReason {
    /// Returns the stable tooling label for this selection reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DirectConcrete => "direct-concrete",
            Self::Qualified => "qualified",
            Self::SoleProviderInWinningSet => "sole-provider-in-winning-set",
            Self::PrimaryProviderInWinningSet => "primary-provider-in-winning-set",
            Self::Collection => "collection",
            Self::Keyed => "keyed",
        }
    }
}

/// Stable runtime stage at which a validated dependency target is selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencySelectionStage {
    /// The requested concrete component itself satisfied the dependency.
    DirectConcrete,
    /// Final provider precedence selected a transient before scope-store lookup.
    TransientPrecedence,
    /// The nearest scope group that could resolve the dependency selected the target.
    ScopePrecedence,
    /// A transient provider was selected after no scope-stored provider resolved.
    TransientFallback,
    /// Fresh construction selected from the nearest eligible scope group.
    FreshScopePrecedence,
    /// A collection retained every eligible provider in final provider order.
    Collection,
    /// Keyed collision precedence selected this provider.
    KeyedPrecedence,
}

impl DependencySelectionStage {
    /// Returns the stable tooling label for this selection stage.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DirectConcrete => "direct-concrete",
            Self::TransientPrecedence => "transient-precedence",
            Self::ScopePrecedence => "scope-precedence",
            Self::TransientFallback => "transient-fallback",
            Self::FreshScopePrecedence => "fresh-scope-precedence",
            Self::Collection => "collection",
            Self::KeyedPrecedence => "keyed-precedence",
        }
    }
}

/// One target retained from the DI engine's scope-aware dependency selection.
#[derive(Clone, Copy, Debug)]
pub struct SelectedDependency {
    /// The selected concrete component or provider target.
    pub target: DependencyTarget,
    /// Stable reason the target participates in this dependency resolution.
    pub reason: DependencySelectionReason,
    /// Stable identity of the selected target's scope, when known.
    pub scope: Option<ScopeId>,
    /// Runtime selection stage that retained this target, when known.
    pub stage: Option<DependencySelectionStage>,
}

/// One provider retained by the shared descriptor selection model.
#[derive(Clone, Copy)]
pub(crate) struct ProviderSelection {
    pub(crate) provider: ProviderDescriptor,
    pub(crate) reason: DependencySelectionReason,
    pub(crate) stage: DependencySelectionStage,
}

/// Immutable provider indexes and selection rules shared by validation, planning,
/// runtime resolution, and tooling projection.
pub struct ProviderSelectionModel {
    components: HashMap<TypeId, ComponentDescriptor>,
    by_trait: HashMap<TypeId, Vec<ProviderDescriptor>>,
    by_concrete: HashMap<TypeId, Vec<ProviderDescriptor>>,
    ordinals: HashMap<TypeId, HashMap<TypeId, usize>>,
}

impl ProviderSelectionModel {
    pub(crate) fn new(
        components: &[ComponentDescriptor],
        mut providers: Vec<ProviderDescriptor>,
        mut ordinals: HashMap<TypeId, HashMap<TypeId, usize>>,
    ) -> crate::Result<Self> {
        validate_provider_components(components, &providers)?;

        let components = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect::<HashMap<_, _>>();
        let mut by_trait = HashMap::<TypeId, Vec<ProviderDescriptor>>::new();
        let mut by_concrete = HashMap::<TypeId, Vec<ProviderDescriptor>>::new();

        for provider in &providers {
            let order = ordinals.entry(provider.trait_ty.type_id).or_default();

            if !order.contains_key(&provider.concrete_ty.type_id) {
                let next = order.values().max().map_or(0, |maximum| maximum + 1);

                order.insert(provider.concrete_ty.type_id, next);
            }
        }

        providers.sort_by_key(|provider| {
            ordinals[&provider.trait_ty.type_id][&provider.concrete_ty.type_id]
        });

        for provider in &providers {
            by_trait
                .entry(provider.trait_ty.type_id)
                .or_default()
                .push(*provider);
            by_concrete
                .entry(provider.concrete_ty.type_id)
                .or_default()
                .push(*provider);
        }

        Ok(Self {
            components,
            by_trait,
            by_concrete,
            ordinals,
        })
    }

    pub(crate) fn providers_for_trait(&self, trait_id: TypeId) -> &[ProviderDescriptor] {
        self.by_trait.get(&trait_id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn providers_for_concrete(&self, concrete: TypeId) -> &[ProviderDescriptor] {
        self.by_concrete.get(&concrete).map_or(&[], Vec::as_slice)
    }

    /// Returns the provider's stable ordinal in the final constrained order.
    pub fn ordinal(&self, provider: &ProviderDescriptor) -> usize {
        self.ordinals[&provider.trait_ty.type_id][&provider.concrete_ty.type_id]
    }

    pub(crate) fn component(&self, type_id: TypeId) -> Option<ComponentDescriptor> {
        self.components.get(&type_id).copied()
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = ComponentDescriptor> + '_ {
        self.components.values().copied()
    }

    pub(crate) fn matching_providers(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
    ) -> Vec<ProviderDescriptor> {
        self.providers_for_trait(trait_id)
            .iter()
            .filter(|provider| provider_matches(provider, qualifier))
            .copied()
            .collect()
    }

    pub(crate) fn has_matching_provider(&self, trait_id: TypeId, qualifier: Option<&str>) -> bool {
        self.providers_for_trait(trait_id)
            .iter()
            .any(|provider| provider_matches(provider, qualifier))
    }

    /// The stable component IDs of every provider of `trait_id` matching `qualifier` that
    /// `consumer` can see and `resolution` can select, in provider order. Deferred
    /// resolution never selects transient providers, so they are left out for it.
    pub(crate) fn visible_provider_ids(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
        resolution: ResolutionMode,
        consumer: &dyn Scope,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
    ) -> Vec<String> {
        self.providers_for_trait(trait_id)
            .iter()
            .filter(|provider| provider_matches(provider, qualifier))
            .filter(|provider| self.is_visible(consumer, provider, can_access))
            .filter_map(|provider| self.component(provider.concrete_ty.type_id))
            .filter(|component| {
                resolution != ResolutionMode::Deferred || !component.scope.is_transient()
            })
            .map(|component| component.id.to_string())
            .collect()
    }

    pub(crate) fn has_visible_provider(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
        consumer: &dyn Scope,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
    ) -> bool {
        self.providers_for_trait(trait_id)
            .iter()
            .filter(|provider| provider_matches(provider, qualifier))
            .any(|provider| self.is_visible(consumer, provider, can_access))
    }

    fn is_transient(&self, provider: &ProviderDescriptor) -> bool {
        self.components[&provider.concrete_ty.type_id]
            .scope
            .is_transient()
    }

    fn is_visible(
        &self,
        consumer: &dyn Scope,
        provider: &ProviderDescriptor,
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
    ) -> bool {
        can_access(
            consumer,
            self.components[&provider.concrete_ty.type_id].scope,
        )
    }
}

#[cfg(test)]
mod tests;
