use std::any::TypeId;

use super::super::{Injectable, ProviderDescriptor};
use super::{BoxedComponent, KeyedStore, SlotStore, from_boxed};
use crate::layout::{RootTarget, SlotIdx, SlotLayout, SlotRef};

/// The instances held by one scope, shared by the under-construction
/// [`ComponentConstructionContext`](crate::ComponentConstructionContext) and the frozen
/// [`ScopeContainer`](crate::ScopeContainer).
///
/// A scope opened from a prepared [`SlotLayout`] stores its instances by slot. The root
/// and scopes opened from a plain construction order store them by type. All lookups are
/// scope-local; walking the parent chain is the caller's job.
pub(crate) enum ScopeStore {
    Keyed(KeyedStore),
    Slots(SlotStore),
}

impl Default for ScopeStore {
    fn default() -> Self {
        Self::Keyed(KeyedStore::default())
    }
}

impl ScopeStore {
    pub(crate) fn resolve_local<H: Injectable>(&self) -> Option<H> {
        match self {
            Self::Keyed(store) => store.resolve_local::<H>(),
            Self::Slots(store) => store.resolve_local::<H>(),
        }
    }

    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        match self {
            Self::Keyed(store) => store.resolve_qualified_local::<H>(qualifier),
            Self::Slots(store) => store.resolve_qualified_local::<H>(qualifier),
        }
    }

    /// Resolves one exact provider descriptor from this scope.
    pub(crate) fn resolve_provider_local<H: Injectable>(
        &self,
        provider: &ProviderDescriptor,
    ) -> Option<H> {
        from_boxed::<H>(self.provider_value(provider)?)
    }

    /// The handle stored at a planned slot. `None` unless this store was opened from
    /// `layout` and the slot is built.
    pub(crate) fn planned<H: Injectable>(&self, layout: &SlotLayout, at: SlotRef) -> Option<H> {
        match self {
            Self::Slots(store) if store.uses(layout) => from_boxed::<H>(store.value(at)?),
            Self::Slots(_) | Self::Keyed(_) => None,
        }
    }

    /// The handle a planned root dependency resolves to.
    pub(crate) fn root<H: Injectable>(&self, target: &RootTarget) -> Option<H> {
        match target {
            RootTarget::Component(type_id) => from_boxed::<H>(self.component(*type_id)?),
            RootTarget::Provider(provider) => self.resolve_provider_local::<H>(provider),
        }
    }

    /// Stores a built or seeded instance.
    pub(crate) fn insert(&mut self, component: BoxedComponent) -> crate::Result<()> {
        match self {
            Self::Keyed(store) => {
                store.insert(component);

                Ok(())
            }

            Self::Slots(store) => store.insert(component),
        }
    }

    /// Stores an instance a planned slot's own factory built, at that slot.
    pub(crate) fn insert_at(
        &mut self,
        slot: SlotIdx,
        component: BoxedComponent,
    ) -> crate::Result<()> {
        match self {
            Self::Slots(store) => store.insert_at(slot, component),

            Self::Keyed(_) => self.insert(component),
        }
    }

    /// The slot store of a scope opened from a layout.
    pub(crate) fn slots(&self) -> Option<&SlotStore> {
        match self {
            Self::Slots(store) => Some(store),
            Self::Keyed(_) => None,
        }
    }

    /// Aliases a built concrete instance under one trait it provides. A slot store
    /// aliases every planned provider when the instance is inserted, so this is a no-op
    /// for it.
    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        if let Self::Keyed(store) = self {
            store.register_provider(provider, ordinal);
        }
    }

    pub(crate) fn contains(&self, type_id: TypeId) -> bool {
        self.component(type_id).is_some()
    }

    pub(crate) fn component(&self, type_id: TypeId) -> Option<&BoxedComponent> {
        match self {
            Self::Keyed(store) => store.components.get(&type_id),
            Self::Slots(store) => store.component(type_id),
        }
    }

    pub(crate) fn provider_value(&self, provider: &ProviderDescriptor) -> Option<&BoxedComponent> {
        match self {
            Self::Keyed(store) => store.provider_value(provider),
            Self::Slots(store) => store.provider_value(provider),
        }
    }

    /// The number of concrete instances held.
    pub(crate) fn component_count(&self) -> usize {
        match self {
            Self::Keyed(store) => store.components.len(),
            Self::Slots(store) => store.component_count(),
        }
    }

    /// The number of provider aliases held.
    #[cfg(test)]
    pub(crate) fn provider_count(&self) -> usize {
        match self {
            Self::Keyed(store) => store.provider_count(),
            Self::Slots(store) => store.provider_count(),
        }
    }
}
