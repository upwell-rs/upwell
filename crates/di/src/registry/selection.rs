use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId};

use super::ComponentRegistry;
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::error::{Error, ProviderComponentMissing};

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

    /// Returns the producer-authoritative targets selected for one dependency.
    ///
    /// Scope reachability has the same meaning as
    /// [`ComponentRegistry::validate_with_scope_reachability`].
    pub fn selected_dependencies_with_scope_reachability(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> Vec<SelectedDependency> {
        self.selected_dependencies(consumer, dependency, &|consumer, dependency| {
            super::scope_allows_with(consumer, dependency, &can_reach)
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

    pub(crate) fn select_dependency(
        &self,
        dependency: &DependencyDescriptor,
        consumer: &dyn Scope,
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
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

    pub(crate) fn construction_providers(
        &self,
        dependency: &DependencyDescriptor,
        consumer: &dyn Scope,
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
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

    pub(crate) fn component(&self, type_id: TypeId) -> Option<ComponentDescriptor> {
        self.components.get(&type_id).copied()
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = ComponentDescriptor> + '_ {
        self.components.values().copied()
    }

    fn selected_dependencies(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
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

    fn select_one(
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

    fn select_collection(
        &self,
        resolution: ResolutionMode,
        qualifier: Option<&str>,
        consumer: &dyn Scope,
        matching: &[ProviderDescriptor],
        can_access: &(impl Fn(&dyn Scope, &'static dyn Scope) -> bool + ?Sized),
    ) -> Vec<ProviderSelection> {
        matching
            .iter()
            .filter(|provider| provider_matches(provider, qualifier))
            .filter(|provider| {
                if resolution == ResolutionMode::Fresh {
                    return self.is_visible(consumer, provider, can_access);
                }

                let component = self.components[&provider.concrete_ty.type_id];

                if component.scope.is_transient() {
                    return resolution != ResolutionMode::Deferred;
                }

                can_access(consumer, component.scope)
            })
            .copied()
            .map(|provider| ProviderSelection {
                provider,
                reason: DependencySelectionReason::Collection,
                stage: DependencySelectionStage::Collection,
            })
            .collect()
    }

    fn select_keyed(
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

pub(super) fn validate_provider_components(
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

pub(super) fn select(
    registry: &ComponentRegistry,
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    components: &[ComponentDescriptor],
    can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
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

fn provider_matches(provider: &ProviderDescriptor, qualifier: Option<&str>) -> bool {
    qualifier.is_none_or(|qualifier| provider.qualifier == qualifier)
}

fn select_provider_where(
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

fn keyed_results(selected: BTreeMap<&'static str, ProviderDescriptor>) -> Vec<ProviderSelection> {
    selected
        .into_values()
        .map(|provider| ProviderSelection {
            provider,
            reason: DependencySelectionReason::Keyed,
            stage: DependencySelectionStage::KeyedPrecedence,
        })
        .collect()
}

fn direct_component_is_selectable(
    component: ComponentDescriptor,
    dependency: &DependencyDescriptor,
    consumer: &dyn Scope,
    can_access: &impl Fn(&dyn Scope, &'static dyn Scope) -> bool,
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

#[cfg(test)]
mod tests;
