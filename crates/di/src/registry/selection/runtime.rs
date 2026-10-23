//! Runtime selection: the providers a resolution from a given scope resolves to.

use std::any::TypeId;
use std::collections::BTreeMap;

use upwell_core::{ResolutionMode, Scope, ScopeId};

use super::rules::{keyed_results, provider_matches, select_provider_where};
use super::{
    DependencySelectionReason, DependencySelectionStage, ProviderSelection, ProviderSelectionModel,
};
use crate::descriptors::ProviderDescriptor;

impl ProviderSelectionModel {
    pub(crate) fn select_runtime_one(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
        resolution: ResolutionMode,
        consumer: &dyn Scope,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
    ) -> Option<ProviderSelection> {
        self.select_one(
            resolution,
            qualifier,
            consumer,
            self.providers_for_trait(trait_id),
            can_access,
        )
    }

    pub(crate) fn select_runtime_collection(
        &self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        consumer: &dyn Scope,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
    ) -> Vec<ProviderSelection> {
        self.select_collection(
            resolution,
            None,
            consumer,
            self.providers_for_trait(trait_id),
            can_access,
        )
    }

    /// Every provider of `trait_id` a runtime collection resolution gathers, without
    /// allocating.
    pub(crate) fn runtime_collection<'a>(
        &'a self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        consumer: &'a dyn Scope,
        can_access: impl Fn(&dyn Scope, &'static dyn Scope) -> bool + 'a,
    ) -> impl Iterator<Item = ProviderDescriptor> + 'a {
        self.collection_providers(
            resolution,
            None,
            consumer,
            self.providers_for_trait(trait_id),
            can_access,
        )
    }

    pub(crate) fn select_runtime_keyed(
        &self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        consumer: &dyn Scope,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
    ) -> Vec<ProviderSelection> {
        self.select_keyed(
            resolution,
            None,
            consumer,
            self.providers_for_trait(trait_id),
            can_access,
        )
    }

    pub(crate) fn visible_groups(
        &self,
        consumer: &dyn Scope,
        providers: &[ProviderDescriptor],
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
        include: impl Fn(&ProviderDescriptor) -> bool,
    ) -> BTreeMap<(u8, ScopeId), Vec<ProviderDescriptor>> {
        let mut groups = BTreeMap::new();

        for provider in providers.iter().filter(|provider| include(provider)) {
            let dependency_scope = self.components[&provider.concrete_ty.type_id].scope;

            if can_access(consumer, dependency_scope) {
                groups
                    .entry((dependency_scope.rank(), dependency_scope.id()))
                    .or_insert_with(Vec::new)
                    .push(*provider);
            }
        }

        groups
    }

    pub(super) fn select_one(
        &self,
        resolution: ResolutionMode,
        qualifier: Option<&str>,
        consumer: &dyn Scope,
        matching: &[ProviderDescriptor],
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
    ) -> Option<ProviderSelection> {
        if resolution == ResolutionMode::Fresh {
            return self.select_from_visible_groups(
                consumer,
                matching,
                qualifier,
                can_access,
                &|provider| provider_matches(provider, qualifier),
                DependencySelectionStage::FreshScopePrecedence,
            );
        }

        if resolution != ResolutionMode::Deferred
            && let Some((provider, reason)) = select_provider_where(matching, qualifier, &|_| true)
            && self.is_transient(&provider)
        {
            return Some(ProviderSelection {
                provider,
                reason,
                stage: DependencySelectionStage::TransientPrecedence,
            });
        }

        if let Some(selected) = self.select_from_visible_groups(
            consumer,
            matching,
            qualifier,
            can_access,
            &|provider| !self.is_transient(provider),
            DependencySelectionStage::ScopePrecedence,
        ) {
            return Some(selected);
        }

        if resolution == ResolutionMode::Deferred {
            return None;
        }

        let (provider, reason) =
            select_provider_where(matching, qualifier, &|provider| self.is_transient(provider))?;

        Some(ProviderSelection {
            provider,
            reason,
            stage: DependencySelectionStage::TransientFallback,
        })
    }

    pub(super) fn select_collection(
        &self,
        resolution: ResolutionMode,
        qualifier: Option<&str>,
        consumer: &dyn Scope,
        matching: &[ProviderDescriptor],
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
    ) -> Vec<ProviderSelection> {
        self.collection_providers(resolution, qualifier, consumer, matching, can_access)
            .map(|provider| ProviderSelection {
                provider,
                reason: DependencySelectionReason::Collection,
                stage: DependencySelectionStage::Collection,
            })
            .collect()
    }

    /// The providers in `matching` a collection dependency gathers, in provider order.
    /// Lazy, so runtime resolution walks them without collecting an intermediate list.
    fn collection_providers<'a>(
        &'a self,
        resolution: ResolutionMode,
        qualifier: Option<&'a str>,
        consumer: &'a dyn Scope,
        matching: &'a [ProviderDescriptor],
        can_access: impl Fn(&dyn Scope, &'static dyn Scope) -> bool + 'a,
    ) -> impl Iterator<Item = ProviderDescriptor> + 'a {
        matching
            .iter()
            .filter(move |provider| provider_matches(provider, qualifier))
            .filter(move |provider| {
                let scope = self.components[&provider.concrete_ty.type_id].scope;

                if resolution != ResolutionMode::Fresh && scope.is_transient() {
                    return resolution != ResolutionMode::Deferred;
                }

                can_access(consumer, scope)
            })
            .copied()
    }

    pub(super) fn select_keyed(
        &self,
        resolution: ResolutionMode,
        qualifier: Option<&str>,
        consumer: &dyn Scope,
        matching: &[ProviderDescriptor],
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
    ) -> Vec<ProviderSelection> {
        let mut selected = BTreeMap::new();

        if resolution == ResolutionMode::Fresh {
            let groups = self.visible_groups(consumer, matching, can_access, |provider| {
                provider_matches(provider, qualifier)
            });

            for providers in groups.values().rev() {
                for provider in providers {
                    selected.insert(provider.qualifier, *provider);
                }
            }

            return keyed_results(selected);
        }

        let groups = self.visible_groups(consumer, matching, can_access, |provider| {
            provider_matches(provider, qualifier) && !self.is_transient(provider)
        });

        for providers in groups.values().rev() {
            for provider in providers {
                selected.insert(provider.qualifier, *provider);
            }
        }

        if resolution != ResolutionMode::Deferred {
            for provider in matching.iter().filter(|provider| {
                provider_matches(provider, qualifier) && self.is_transient(provider)
            }) {
                selected.insert(provider.qualifier, *provider);
            }
        }

        keyed_results(selected)
    }

    fn select_from_visible_groups(
        &self,
        consumer: &dyn Scope,
        providers: &[ProviderDescriptor],
        qualifier: Option<&str>,
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
        include: &impl Fn(&ProviderDescriptor) -> bool,
        stage: DependencySelectionStage,
    ) -> Option<ProviderSelection> {
        let mut after = None;

        loop {
            let key = providers
                .iter()
                .filter(|provider| include(provider))
                .filter_map(|provider| {
                    let scope = self.components[&provider.concrete_ty.type_id].scope;
                    let key = (scope.rank(), scope.id());

                    (can_access(consumer, scope) && after.is_none_or(|after| key > after))
                        .then_some(key)
                })
                .min()?;

            if let Some((provider, reason)) =
                select_provider_where(providers, qualifier, &|provider| {
                    if !include(provider) {
                        return false;
                    }

                    let scope = self.components[&provider.concrete_ty.type_id].scope;
                    can_access(consumer, scope) && (scope.rank(), scope.id()) == key
                })
            {
                return Some(ProviderSelection {
                    provider,
                    reason,
                    stage,
                });
            }

            after = Some(key);
        }
    }
}
