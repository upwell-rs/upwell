use std::any::TypeId;
use std::ptr;
use std::sync::{Arc, OnceLock};

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
/// open scope allocates only its values. Each slot is set once: in `values` while the scope
/// opens, or in `later` when a slot its opening did not build is built on demand. The second
/// set is allocated by the first such build, so a scope whose opening built everything it
/// uses reads plain values.
pub(crate) struct SlotStore {
    layout: Arc<SlotLayout>,
    values: Box<[Option<SlotValue>]>,
    later: OnceLock<Box<Later>>,
}

/// The slots of an open scope built on demand, and the lock their builds hold so each is
/// built once.
struct Later {
    building: tokio::sync::Mutex<()>,
    values: Box<[OnceLock<SlotValue>]>,
}

impl SlotStore {
    pub(crate) fn new(layout: Arc<SlotLayout>) -> Self {
        let values = layout.slots().iter().map(|_| None).collect();

        Self {
            layout,
            values,
            later: OnceLock::new(),
        }
    }

    /// Whether this store was opened from `layout`.
    pub(crate) fn uses(&self, layout: &SlotLayout) -> bool {
        ptr::eq(Arc::as_ptr(&self.layout), layout)
    }

    /// The layout this store was opened from.
    pub(crate) fn layout(&self) -> &Arc<SlotLayout> {
        &self.layout
    }

    /// Whether `slot` holds its instance.
    pub(crate) fn is_built(&self, slot: SlotIdx) -> bool {
        self.built(slot).is_some()
    }

    fn built(&self, slot: SlotIdx) -> Option<&SlotValue> {
        let index = slot.index();

        match &self.values[index] {
            Some(value) => Some(value),
            None => self.later.get()?.values[index].get(),
        }
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

        self.insert_at(slot, component)
    }

    /// Stores an instance a slot's own factory built while the scope opens, at that slot.
    ///
    /// A factory returning another component than its slot holds is rejected before the
    /// instance is aliased under the slot's providers, and so is a second instance for a
    /// slot that already holds one.
    pub(crate) fn insert_at(
        &mut self,
        slot: SlotIdx,
        component: BoxedComponent,
    ) -> crate::Result<()> {
        if self.is_built(slot) {
            return Err(self.built_twice(slot));
        }

        self.values[slot.index()] = Some(self.slot_value(slot, component)?);

        Ok(())
    }

    /// Stores an instance built on demand in the open scope, at its slot.
    ///
    /// The checks are those of [`insert_at`](Self::insert_at).
    pub(crate) fn insert_built(
        &self,
        slot: SlotIdx,
        component: BoxedComponent,
    ) -> crate::Result<()> {
        if self.values[slot.index()].is_some() {
            return Err(self.built_twice(slot));
        }

        self.later().values[slot.index()]
            .set(self.slot_value(slot, component)?)
            .map_err(|_| self.built_twice(slot))
    }

    /// The stored value of `component`, aliased under every trait `slot` provides.
    fn slot_value(&self, slot: SlotIdx, component: BoxedComponent) -> crate::Result<SlotValue> {
        let spec = self.layout.spec(slot);

        if spec.component.ty.type_id != component.ty.type_id {
            return Err(Error::FactoryOutputMismatch {
                component: spec.component.name,
                built: component.ty.name,
            });
        }

        let providers = spec
            .providers
            .iter()
            .map(|provider| (provider.erase)(&component))
            .collect();

        Ok(SlotValue {
            component,
            providers,
        })
    }

    /// Holds the on-demand build lock of this store until the guard drops.
    pub(crate) async fn lock_builds(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.later().building.lock().await
    }

    fn later(&self) -> &Later {
        self.later.get_or_init(|| {
            Box::new(Later {
                building: tokio::sync::Mutex::new(()),
                values: self.values.iter().map(|_| OnceLock::new()).collect(),
            })
        })
    }

    fn built_twice(&self, slot: SlotIdx) -> Error {
        Error::SlotBuiltTwice {
            component: self.layout.spec(slot).component.name,
            scope: self.layout.scope().name(),
        }
    }

    /// The value at `at`, if its slot is built.
    pub(crate) fn value(&self, at: SlotRef) -> Option<&BoxedComponent> {
        let built = self.built(at.slot)?;

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
        (0..self.values.len())
            .filter(|index| self.is_built(SlotIdx::new(*index)))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn provider_count(&self) -> usize {
        (0..self.values.len())
            .filter_map(|index| self.built(SlotIdx::new(index)))
            .map(|built| built.providers.len())
            .sum()
    }

    fn built_providers(&self, trait_id: TypeId) -> impl Iterator<Item = &ProviderSlot> {
        self.layout
            .providers_of(trait_id)
            .iter()
            .filter(|provider| self.is_built(provider.at.slot))
    }
}
