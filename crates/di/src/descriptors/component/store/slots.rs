use std::any::TypeId;
use std::ptr;
use std::sync::Arc;

use super::super::{Injectable, ProviderDescriptor};
use super::{BoxedComponent, from_boxed};
use crate::error::Error;
use crate::layout::{ProviderSlot, SlotIdx, SlotLayout, SlotRef};

/// One built slot: the concrete instance and its provider aliases, in the order of the
/// slot's planned providers.
struct SlotValue {
    component: BoxedComponent,
    providers: Box<[BoxedComponent]>,
}

/// The instances of a scope opened from a prepared [`SlotLayout`], stored densely by slot.
///
/// The layout's type and trait indexes are shared by every instance of the scope, so an
/// open scope allocates only its values.
pub(crate) struct SlotStore {
    layout: Arc<SlotLayout>,
    values: Box<[Option<SlotValue>]>,
}

impl SlotStore {
    pub(crate) fn new(layout: Arc<SlotLayout>) -> Self {
        let values = layout.slots().iter().map(|_| None).collect();

        Self { layout, values }
    }

    /// Whether this store was opened from `layout`.
    pub(crate) fn uses(&self, layout: &SlotLayout) -> bool {
        ptr::eq(Arc::as_ptr(&self.layout), layout)
    }

    /// Stores a built or seeded instance in its slot and aliases it under every trait the
    /// slot provides.
    pub(crate) fn insert(&mut self, component: BoxedComponent) -> crate::Result<()> {
        let slot = self
            .layout
            .slot_of(component.ty.type_id)
            .ok_or(Error::UnplannedComponent {
                component: component.ty.name,
                scope: self.layout.scope().name(),
            })?;

        self.insert_at(slot, component);

        Ok(())
    }

    /// Stores an instance a slot's own factory built, at that slot.
    pub(crate) fn insert_at(&mut self, slot: SlotIdx, component: BoxedComponent) {
        debug_assert_eq!(
            self.layout.slot_of(component.ty.type_id),
            Some(slot),
            "a factory built another component than its slot holds"
        );

        let providers = self
            .layout
            .spec(slot)
            .providers
            .iter()
            .map(|provider| (provider.erase)(&component))
            .collect();

        self.values[slot.index()] = Some(SlotValue {
            component,
            providers,
        });
    }

    /// The value at `at`, if its slot is built.
    pub(crate) fn value(&self, at: SlotRef) -> Option<&BoxedComponent> {
        let built = self.values[at.slot.index()].as_ref()?;

        match at.alias {
            Some(alias) => built.providers.get(alias),
            None => Some(&built.component),
        }
    }

    pub(crate) fn component(&self, type_id: TypeId) -> Option<&BoxedComponent> {
        self.value(SlotRef {
            slot: self.layout.slot_of(type_id)?,
            alias: None,
        })
    }

    /// Single concrete-or-primary-provider lookup over the built slots. `None` if absent
    /// or ambiguous.
    pub(crate) fn resolve_local<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();

        if let Some(component) = self.component(type_id) {
            return from_boxed::<H>(component);
        }

        let built: Vec<&ProviderSlot> = self.built_providers(type_id).collect();
        let chosen = crate::registry::selection::select_single_by(&built, |provider| {
            provider.descriptor.primary
        })?;

        from_boxed::<H>(self.value(chosen.at)?)
    }

    /// Qualifier-selected single provider over the built slots.
    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let mut built = self.built_providers(type_id);
        let chosen = built.find(|provider| provider.descriptor.qualifier == qualifier)?;

        from_boxed::<H>(self.value(chosen.at)?)
    }

    /// The erased value of one exact provider descriptor.
    pub(crate) fn provider_value(&self, provider: &ProviderDescriptor) -> Option<&BoxedComponent> {
        let chosen = self
            .layout
            .providers_of(provider.trait_ty.type_id)
            .iter()
            .find(|candidate| {
                candidate.descriptor.concrete_ty.type_id == provider.concrete_ty.type_id
                    && candidate.descriptor.qualifier == provider.qualifier
            })?;

        self.value(chosen.at)
    }

    pub(crate) fn component_count(&self) -> usize {
        self.values.iter().flatten().count()
    }

    #[cfg(test)]
    pub(crate) fn provider_count(&self) -> usize {
        self.values
            .iter()
            .flatten()
            .map(|built| built.providers.len())
            .sum()
    }

    fn built_providers(&self, trait_id: TypeId) -> impl Iterator<Item = &ProviderSlot> {
        self.layout
            .providers_of(trait_id)
            .iter()
            .filter(|provider| self.values[provider.at.slot.index()].is_some())
    }
}
