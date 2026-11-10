use std::any::TypeId;
use std::collections::HashMap;

use super::super::{Injectable, ProviderDescriptor};
use super::{BoxedComponent, from_boxed};

/// One trait provider's constructed value, plus the metadata that selects it.
pub(crate) struct ProviderInstance {
    pub(crate) descriptor: ProviderDescriptor,
    pub(crate) ordinal: usize,
    pub(crate) value: BoxedComponent,
}

/// The instances and trait providers of a scope without a prepared slot layout, keyed by
/// type.
///
/// The root container and scopes opened directly from a construction order use it. All
/// lookups here are scope-local; walking the parent chain is the caller's job. Config
/// values are not held here: config is an external resolver, not part of the container.
#[derive(Default)]
pub(crate) struct KeyedStore {
    pub(crate) components: HashMap<TypeId, BoxedComponent>,
    pub(crate) providers: HashMap<TypeId, Vec<ProviderInstance>>,
}

impl KeyedStore {
    /// Single concrete-or-primary-provider lookup. `None` if absent or ambiguous.
    pub(crate) fn resolve_local<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();

        if let Some(component) = self.components.get(&type_id) {
            return from_boxed::<H>(component);
        }

        let chosen =
            crate::registry::selection::select_single_by(self.providers.get(&type_id)?, |entry| {
                entry.descriptor.primary
            })?;

        from_boxed::<H>(&chosen.value)
    }

    /// Qualifier-selected single provider.
    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let entry = self
            .providers
            .get(&type_id)?
            .iter()
            .find(|entry| entry.descriptor.qualifier == qualifier)?;

        from_boxed::<H>(&entry.value)
    }

    /// The erased value of one exact provider descriptor.
    pub(crate) fn provider_value(&self, provider: &ProviderDescriptor) -> Option<&BoxedComponent> {
        let entry = self
            .providers
            .get(&provider.trait_ty.type_id)?
            .iter()
            .find(|entry| {
                entry.descriptor.concrete_ty.type_id == provider.concrete_ty.type_id
                    && entry.descriptor.qualifier == provider.qualifier
            })?;

        Some(&entry.value)
    }

    pub(crate) fn insert(&mut self, component: BoxedComponent) {
        let type_id = component.ty.type_id;

        self.components.insert(type_id, component);
    }

    /// Erases an already-constructed concrete component as `Arc<dyn Trait>` and
    /// records it under the trait's key, so trait-object dependencies resolve.
    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        let concrete_id = provider.concrete_ty.type_id;

        let Some(concrete) = self.components.get(&concrete_id) else {
            return;
        };

        let erased = (provider.erase)(concrete);
        let providers = self.providers.entry(provider.trait_ty.type_id).or_default();

        providers.push(ProviderInstance {
            descriptor: *provider,
            ordinal,
            value: erased,
        });
        providers.sort_by_key(|provider| provider.ordinal);
    }

    #[cfg(test)]
    pub(crate) fn provider_count(&self) -> usize {
        self.providers.values().map(Vec::len).sum()
    }
}
