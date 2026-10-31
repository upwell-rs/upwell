//! Provider selection rules shared by validation and runtime: primary and qualifier
//! matching, and provider-component checks.

use std::collections::BTreeMap;

use upwell_core::{DependencyDescriptor, ResolutionMode, ScopeKey};

use super::{
    DependencySelectionReason, DependencySelectionStage, ProviderSelection, ProviderSelectionModel,
    SelectedDependency,
};
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::error::{Error, ProviderComponentMissing};
use crate::registry::ComponentRegistry;

pub(crate) fn validate_provider_components(
    components: &[ComponentDescriptor],
    providers: &[ProviderDescriptor],
) -> crate::Result<()> {
    let component_types = components
        .iter()
        .map(|component| component.ty.type_id)
        .collect::<std::collections::HashSet<_>>();

    for provider in providers {
        if !component_types.contains(&provider.concrete_ty.type_id) {
            return Err(Error::ProviderComponentMissing(Box::new(
                ProviderComponentMissing {
                    trait_name: provider.trait_ty.name.to_string(),
                    trait_type: (provider.trait_ty.type_name)().to_string(),
                    component: provider.concrete_ty.name.to_string(),
                    component_type: (provider.concrete_ty.type_name)().to_string(),
                    qualifier: provider.qualifier.to_string(),
                },
            )));
        }
    }

    Ok(())
}

pub(crate) fn select(
    registry: &ComponentRegistry,
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    components: &[ComponentDescriptor],
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> crate::Result<Vec<SelectedDependency>> {
    let order = registry.provider_order(components)?;
    let model = ProviderSelectionModel::new(components, registry.providers.clone(), order)?;

    Ok(model.selected_dependencies(consumer, dependency, can_access))
}

pub(crate) fn select_single_provider(
    providers: &[ProviderDescriptor],
) -> Option<ProviderDescriptor> {
    select_single_by(providers, |provider| provider.primary).copied()
}

pub(crate) fn select_single_by<T>(items: &[T], is_primary: impl Fn(&T) -> bool) -> Option<&T> {
    if items.len() == 1 {
        return items.first();
    }

    let mut primaries = items.iter().filter(|item| is_primary(item));
    let primary = primaries.next()?;

    if primaries.next().is_some() {
        return None;
    }

    Some(primary)
}

pub(crate) fn select_provider(
    providers: &[ProviderDescriptor],
    qualifier: Option<&str>,
) -> Option<ProviderDescriptor> {
    match qualifier {
        Some(qualifier) => providers
            .iter()
            .find(|provider| provider.qualifier == qualifier)
            .copied(),
        None => select_single_provider(providers),
    }
}

pub(crate) fn provider_matches(provider: &ProviderDescriptor, qualifier: Option<&str>) -> bool {
    qualifier.is_none_or(|qualifier| provider.qualifier == qualifier)
}

pub(crate) fn select_provider_where(
    providers: &[ProviderDescriptor],
    qualifier: Option<&str>,
    include: &impl Fn(&ProviderDescriptor) -> bool,
) -> Option<(ProviderDescriptor, DependencySelectionReason)> {
    let mut matching = providers
        .iter()
        .filter(|provider| provider_matches(provider, qualifier) && include(provider));
    let first = *matching.next()?;

    if qualifier.is_some() {
        return Some((first, DependencySelectionReason::Qualified));
    }

    let second = matching.next();
    if second.is_none() {
        return Some((first, DependencySelectionReason::SoleProviderInWinningSet));
    }

    let mut primary = first.primary.then_some(first);
    for provider in second.into_iter().chain(matching) {
        if provider.primary {
            if primary.is_some() {
                return None;
            }
            primary = Some(*provider);
        }
    }

    primary.map(|provider| {
        (
            provider,
            DependencySelectionReason::PrimaryProviderInWinningSet,
        )
    })
}

pub(crate) fn keyed_results(
    selected: BTreeMap<&'static str, ProviderDescriptor>,
) -> Vec<ProviderSelection> {
    selected
        .into_values()
        .map(|provider| ProviderSelection {
            provider,
            reason: DependencySelectionReason::Keyed,
            stage: DependencySelectionStage::KeyedPrecedence,
        })
        .collect()
}

pub(crate) fn direct_component_is_selectable(
    component: ComponentDescriptor,
    dependency: &DependencyDescriptor,
    consumer: ScopeKey,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> bool {
    match dependency.resolution {
        ResolutionMode::Deferred => !component.scope.is_transient(),
        ResolutionMode::Fresh => {
            can_access(consumer, component.scope)
                && component
                    .effective_factory()
                    .is_ok_and(|factory| factory.is_some())
        }
        ResolutionMode::Eager | ResolutionMode::Lazy => true,
    }
}
