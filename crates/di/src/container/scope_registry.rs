//! The immutable per-generation registry every scope container of one root shares.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolutionMode, Scope};

use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::error::Error;
use crate::registry::selection::ProviderSelectionModel;

/// Shared, immutable data a [`ScopeContainer`] needs to resolve beyond its own
/// store: the `Transient` components it may construct on demand and the trait
/// providers used to alias instances. Held behind an `Arc` and shared by every
/// scope in an application (root, per-connection, per-request).
pub struct ScopeRegistry {
    /// Transient components keyed by their concrete `TypeId`, for on-demand
    /// construction at resolution time. Transients are never cached.
    transient: HashMap<TypeId, ComponentDescriptor>,
    factory_backed: HashMap<TypeId, ComponentDescriptor>,
    by_id: HashMap<&'static str, TypeId>,
    pub(super) selection: Arc<ProviderSelectionModel>,
}

impl ScopeRegistry {
    pub fn new(
        transient: HashMap<TypeId, ComponentDescriptor>,
        components: HashMap<TypeId, ComponentDescriptor>,
        providers: Vec<ProviderDescriptor>,
        provider_order: HashMap<TypeId, HashMap<TypeId, usize>>,
    ) -> crate::Result<Self> {
        let selection_components = components
            .values()
            .copied()
            .chain(transient.values().copied())
            .collect::<Vec<_>>();
        let selection = Arc::new(ProviderSelectionModel::new(
            &selection_components,
            providers,
            provider_order,
        )?);

        Self::from_selection_model(transient, components, selection)
    }

    /// Creates a runtime registry from the provider model retained by validation
    /// and construction planning.
    pub fn from_selection_model(
        transient: HashMap<TypeId, ComponentDescriptor>,
        components: HashMap<TypeId, ComponentDescriptor>,
        selection: Arc<ProviderSelectionModel>,
    ) -> crate::Result<Self> {
        let factory_backed = components
            .values()
            .filter(|component| component.effective_factory().ok().flatten().is_some())
            .map(|component| (component.ty.type_id, *component))
            .collect();
        let by_id = selection
            .components()
            .map(|component| (component.id, component.ty.type_id))
            .collect();

        for component in transient.values().chain(components.values()) {
            if selection.component(component.ty.type_id).is_none() {
                return Err(Error::MissingComponent(component.name));
            }
        }

        Ok(Self {
            transient,
            factory_backed,
            by_id,
            selection,
        })
    }

    pub(crate) fn providers_for(&self, concrete: TypeId) -> &[ProviderDescriptor] {
        self.selection.providers_for_concrete(concrete)
    }

    pub(crate) fn selected_runtime_provider(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
        resolution: ResolutionMode,
        scope: &dyn Scope,
        can_access: &impl Fn(&'static dyn Scope) -> bool,
    ) -> Option<ProviderDescriptor> {
        self.selection
            .select_runtime_one(trait_id, qualifier, resolution, scope, &|_, dependency| {
                can_access(dependency)
            })
            .map(|selection| selection.provider)
    }

    pub(crate) fn selected_runtime_collection<'a>(
        &'a self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        scope: &'a dyn Scope,
        can_access: impl Fn(&'static dyn Scope) -> bool + 'a,
    ) -> impl Iterator<Item = ProviderDescriptor> + 'a {
        self.selection
            .runtime_collection(trait_id, resolution, scope, move |_, dependency| {
                can_access(dependency)
            })
    }

    pub(crate) fn selected_runtime_keyed(
        &self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        scope: &dyn Scope,
        can_access: &impl Fn(&'static dyn Scope) -> bool,
    ) -> Vec<ProviderDescriptor> {
        self.selection
            .select_runtime_keyed(trait_id, resolution, scope, &|_, dependency| {
                can_access(dependency)
            })
            .into_iter()
            .map(|selection| selection.provider)
            .collect()
    }

    pub(crate) fn provider_ordinal(&self, provider: &ProviderDescriptor) -> usize {
        self.selection.ordinal(provider)
    }

    pub(crate) fn transient(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.transient.get(&target).copied()
    }

    pub(crate) fn factory_backed(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.factory_backed.get(&target).copied()
    }

    pub(crate) fn component(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.selection.component(target)
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = ComponentDescriptor> + '_ {
        self.selection.components()
    }

    pub(crate) fn component_by_id(&self, id: &str) -> Option<ComponentDescriptor> {
        self.by_id
            .get(id)
            .and_then(|type_id| self.selection.component(*type_id))
    }
}
