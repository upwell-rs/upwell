use std::any::TypeId;
use std::future::Future;
use std::ptr;
use std::sync::{Arc, OnceLock};

use super::super::{Injectable, ProviderDescriptor};
use super::{BoxedComponent, from_boxed};
use crate::error::Error;
use crate::layout::{
    ContainerKind, EntryPlan, ProviderSlot, SlotIdx, SlotLayout, SlotPositions, SlotRef,
};

/// The instances of a scope opened from a prepared [`SlotLayout`], stored densely by slot.
///
/// The layout's type and trait indexes are shared by every instance of the scope, so an
/// open scope allocates only its values, in one block laid out by the layout's
/// [`ContainerKind`]. Each slot is set once. `declares`, when set, lists the only slots that
/// may be built on demand.
///
/// A slot holds only its concrete instance. A read through one of its provider aliases erases
/// the instance under that trait with the provider's [`erase`](ProviderDescriptor::erase), so
/// an alias nothing reads costs nothing, and a fixed instance needs no shared alias cell.
pub(crate) struct SlotStore {
    layout: Arc<SlotLayout>,
    slots: Slots,
    declares: Option<Arc<[bool]>>,
}

/// The slots of a [`SlotStore`], in the layout of its [`ContainerKind`].
enum Slots {
    /// Plain values set while the scope opens. `values` holds only the slots the opening fills,
    /// at the places its entry plan's `positions` gives them, so its size follows the plan
    /// rather than the scope's component count; without a plan it holds every slot. Any other
    /// slot is built into `later`, allocated by the first such build, so a scope whose opening
    /// built everything it uses reads plain values.
    Pinned {
        values: Box<[Option<BoxedComponent>]>,
        positions: Option<Arc<SlotPositions>>,
        later: OnceLock<Cells>,
    },
    /// One once-cell per slot, allocated when the scope opens, whatever the opening's plan: a
    /// long-lived scope builds most of its slots on first use, concurrently, so every slot
    /// needs its cell, and one such block is opened per connection rather than per call.
    Durable(Cells),
}

/// Slots built after their scope opened. Each cell is initialized by one build, which
/// concurrent requests for the same slot wait on, while other slots build independently. A
/// failed build leaves its cell empty.
type Cells = Box<[tokio::sync::OnceCell<BoxedComponent>]>;

impl SlotStore {
    /// An empty store for an instance opened from `layout` with `plan`, or filling every slot
    /// without one.
    pub(crate) fn new(layout: Arc<SlotLayout>, plan: Option<&EntryPlan>) -> Self {
        let slots = match layout.kind() {
            ContainerKind::Pinned => {
                let positions = plan.map(|plan| Arc::clone(plan.positions()));
                let places = positions
                    .as_ref()
                    .map_or(layout.len(), |positions| positions.len());

                Slots::Pinned {
                    values: (0..places).map(|_| None).collect(),
                    positions,
                    later: OnceLock::new(),
                }
            }
            ContainerKind::Durable => Slots::Durable(cells(layout.len())),
        };

        Self {
            layout,
            slots,
            declares: plan.and_then(EntryPlan::declares).cloned(),
        }
    }

    /// The container kind this store is laid out for.
    #[cfg(test)]
    pub(crate) fn kind(&self) -> ContainerKind {
        match self.slots {
            Slots::Pinned { .. } => ContainerKind::Pinned,
            Slots::Durable(_) => ContainerKind::Durable,
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

    fn built(&self, slot: SlotIdx) -> Option<&BoxedComponent> {
        let index = slot.index();

        match &self.slots {
            Slots::Pinned {
                values,
                positions,
                later,
            } => match place(positions.as_deref(), slot).and_then(|at| values[at].as_ref()) {
                Some(value) => Some(value),
                None => later.get()?[index].get(),
            },
            Slots::Durable(cells) => cells[index].get(),
        }
    }

    /// Stores a built or seeded instance in its slot.
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
    /// A factory returning another component than its slot holds is rejected, and so is a
    /// second instance for a slot that already holds one.
    pub(crate) fn insert_at(
        &mut self,
        slot: SlotIdx,
        component: BoxedComponent,
    ) -> crate::Result<()> {
        if self.is_built(slot) {
            return Err(self.built_twice(slot));
        }

        let value = self.checked(slot, component)?;
        let len = self.layout.len();

        match &mut self.slots {
            Slots::Pinned {
                values,
                positions,
                later,
            } => match place(positions.as_deref(), slot) {
                Some(at) => values[at] = Some(value),
                None => {
                    // The check above and exclusive access leave this cell empty.
                    let _ = later.get_or_init(|| cells(len))[slot.index()].set(value);
                }
            },
            Slots::Durable(cells) => {
                cells[slot.index()] = tokio::sync::OnceCell::new_with(Some(value));
            }
        }

        Ok(())
    }

    /// Builds `slot` in the open scope with `build`, unless it holds its instance already.
    ///
    /// Concurrent calls for the same slot run one build and wait for it. The output is checked
    /// as [`insert_at`](Self::insert_at) checks it. A slot the store does not declare is
    /// rejected before it is built.
    pub(crate) async fn build_later(
        &self,
        slot: SlotIdx,
        build: impl Future<Output = crate::Result<BoxedComponent>>,
    ) -> crate::Result<()> {
        if self.is_built(slot) {
            return Ok(());
        }

        if self
            .declares
            .as_ref()
            .is_some_and(|declares| !declares[slot.index()])
        {
            return Err(Error::UndeclaredConstruction {
                component: self.layout.spec(slot).component.name,
                scope: self.layout.scope().name(),
            });
        }

        self.later()[slot.index()]
            .get_or_try_init(|| async { self.checked(slot, build.await?) })
            .await?;

        Ok(())
    }

    /// Fills the seed slot `slot` of the open scope with `component`, unless it holds a value.
    pub(crate) fn seed_later(&self, slot: SlotIdx, component: BoxedComponent) -> crate::Result<()> {
        let value = self.checked(slot, component)?;

        // A value set between the caller's check and here came from the same request head.
        let _ = self.later()[slot.index()].set(value);

        Ok(())
    }

    /// `component`, once it is known to be the instance `slot` holds.
    fn checked(&self, slot: SlotIdx, component: BoxedComponent) -> crate::Result<BoxedComponent> {
        let spec = self.layout.spec(slot);

        if spec.component.ty.type_id != component.ty.type_id {
            return Err(Error::FactoryOutputMismatch {
                component: spec.component.name,
                built: component.ty.name,
            });
        }

        Ok(component)
    }

    /// The cells slots built after the scope opened go in.
    fn later(&self) -> &Cells {
        match &self.slots {
            Slots::Pinned { later, .. } => later.get_or_init(|| cells(self.layout.len())),
            Slots::Durable(cells) => cells,
        }
    }

    fn built_twice(&self, slot: SlotIdx) -> Error {
        Error::SlotBuiltTwice {
            component: self.layout.spec(slot).component.name,
            scope: self.layout.scope().name(),
        }
    }

    /// The handle at `at`, if its slot is built, erased under the alias `at` names.
    pub(crate) fn read<H: Injectable>(&self, at: SlotRef) -> Option<H> {
        let built = self.built(at.slot)?;

        match at.alias {
            Some(alias) => {
                let provider = self.layout.spec(at.slot).providers.get(alias)?;

                from_boxed::<H>(&(provider.erase)(built))
            }

            None => from_boxed::<H>(built),
        }
    }

    pub(crate) fn component(&self, type_id: TypeId) -> Option<&BoxedComponent> {
        self.built(self.layout.slot_of(type_id)?)
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

        self.read::<H>(chosen.at)
    }

    /// Qualifier-selected single provider over the built slots.
    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let mut built = self.built_providers(type_id);
        let chosen = built.find(|provider| provider.descriptor.qualifier == qualifier)?;

        self.read::<H>(chosen.at)
    }

    /// The handle of one exact provider descriptor.
    pub(crate) fn resolve_provider<H: Injectable>(
        &self,
        provider: &ProviderDescriptor,
    ) -> Option<H> {
        let chosen = self
            .layout
            .providers_of(provider.trait_ty.type_id)
            .iter()
            .find(|candidate| {
                candidate.descriptor.concrete_ty.type_id == provider.concrete_ty.type_id
                    && candidate.descriptor.qualifier == provider.qualifier
            })?;

        self.read::<H>(chosen.at)
    }

    pub(crate) fn component_count(&self) -> usize {
        (0..self.layout.len())
            .filter(|index| self.is_built(SlotIdx::new(*index)))
            .count()
    }

    fn built_providers(&self, trait_id: TypeId) -> impl Iterator<Item = &ProviderSlot> {
        self.layout
            .providers_of(trait_id)
            .iter()
            .filter(|provider| self.is_built(provider.at.slot))
    }
}

/// The place of `slot` in a pinned store's values: where `positions` puts it, or its own index
/// in a store holding every slot.
fn place(positions: Option<&SlotPositions>, slot: SlotIdx) -> Option<usize> {
    match positions {
        Some(positions) => positions.place(slot),
        None => Some(slot.index()),
    }
}

/// `len` empty once-cells.
fn cells(len: usize) -> Cells {
    (0..len).map(|_| tokio::sync::OnceCell::new()).collect()
}
