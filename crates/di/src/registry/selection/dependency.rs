//! Validation-time selection: the providers each declared dependency edge resolves to.

use std::any::TypeId;

use upwell_core::{Cardinality, DependencyDescriptor, ScopeId, ScopeKey};

use super::rules::{direct_component_is_selectable, provider_matches, select_provider};
use super::{
    DependencySelectionReason, DependencySelectionStage, DependencyTarget, ProviderSelection,
    ProviderSelectionModel, SelectedDependency,
};
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};

impl ProviderSelectionModel {
    /// Returns the producer-authoritative targets selected for one dependency.
    ///
    /// Scope reachability has the same meaning as
    /// [`ComponentRegistry::validate_with_scope_reachability`](crate::ComponentRegistry::validate_with_scope_reachability).
    pub fn selected_dependencies_with_scope_reachability(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> Vec<SelectedDependency> {
        self.selected_dependencies(consumer, dependency, &|consumer, dependency| {
            crate::registry::scopes::scope_allows_with(consumer, dependency, &can_reach)
        })
    }

    #[cfg(test)]
    pub(crate) fn select_global(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
    ) -> Option<ProviderDescriptor> {
        select_provider(self.providers_for_trait(trait_id), qualifier)
    }

    fn select_dependency(
        &self,
        dependency: &DependencyDescriptor,
        consumer: ScopeKey,
        can_access: &(impl Fn(ScopeKey, ScopeKey) -> bool + ?Sized),
    ) -> Vec<ProviderSelection> {
        let providers = self.providers_for_trait(dependency.ty.type_id);

        match dependency.cardinality {
            Cardinality::One => self
                .select_one(
                    dependency.resolution,
                    dependency.qualifier,
                    consumer,
                    providers,
                    can_access,
                )
                .into_iter()
                .collect(),
            Cardinality::Collection => self.select_collection(
                dependency.resolution,
                dependency.qualifier,
                consumer,
                providers,
                can_access,
            ),
            Cardinality::Keyed => self.select_keyed(
                dependency.resolution,
                dependency.qualifier,
                consumer,
                providers,
                can_access,
            ),
        }
    }

    pub(super) fn selected_dependencies(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    ) -> Vec<SelectedDependency> {
        if dependency.config {
            crate::observability::selection_external(consumer, dependency, "config-external");

            return Vec::new();
        }

        if dependency.dynamic {
            crate::observability::selection_external(consumer, dependency, "dynamic-runtime");

            return Vec::new();
        }

        if let Some(component) = self.component(dependency.ty.type_id)
            && direct_component_is_selectable(component, dependency, consumer.scope, can_access)
        {
            crate::observability::direct_selection(consumer, dependency, component);

            return vec![SelectedDependency {
                target: DependencyTarget::Component(component),
                reason: DependencySelectionReason::DirectConcrete,
                scope: Some(component.scope.id()),
                stage: Some(DependencySelectionStage::DirectConcrete),
            }];
        }

        let provider_selections = self.select_dependency(dependency, consumer.scope, can_access);

        for selection in &provider_selections {
            let component = self.components[&selection.provider.concrete_ty.type_id];

            crate::observability::provider_selection(
                consumer,
                dependency,
                selection.provider,
                component,
                selection.reason,
                selection.stage,
            );
        }

        let selected = provider_selections
            .into_iter()
            .map(|selection| SelectedDependency {
                target: DependencyTarget::Provider(selection.provider),
                reason: selection.reason,
                scope: self
                    .component(selection.provider.concrete_ty.type_id)
                    .map(|component| component.scope.id()),
                stage: Some(selection.stage),
            })
            .collect::<Vec<_>>();

        if selected.is_empty() {
            crate::observability::selection_absent(consumer, dependency);
        }

        selected
    }

    pub(crate) fn construction_providers(
        &self,
        dependency: &DependencyDescriptor,
        consumer: ScopeKey,
        can_access: &(impl Fn(ScopeKey, ScopeKey) -> bool + ?Sized),
        is_pending: &impl Fn(TypeId) -> bool,
    ) -> Vec<ProviderDescriptor> {
        let selected = self.select_dependency(dependency, consumer, can_access);
        let mut providers = selected
            .iter()
            .map(|selection| selection.provider)
            .collect::<Vec<_>>();

        if dependency.cardinality == Cardinality::One
            && selected.first().is_some_and(|selection| {
                selection.stage == DependencySelectionStage::ScopePrecedence
            })
        {
            let candidates = self.providers_for_trait(dependency.ty.type_id);
            let groups = self.visible_groups(consumer, candidates, can_access, |provider| {
                provider_matches(provider, dependency.qualifier) && !self.is_transient(provider)
            });

            for group in groups.values() {
                if select_provider(group, dependency.qualifier).is_some() {
                    break;
                }

                providers.extend(
                    group
                        .iter()
                        .filter(|provider| is_pending(provider.concrete_ty.type_id))
                        .copied(),
                );
            }
        }

        providers.sort_by_key(|provider| self.ordinal(provider));
        providers.dedup_by_key(|provider| provider.concrete_ty.type_id);

        providers
    }
}
