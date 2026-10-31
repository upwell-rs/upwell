use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::ScopeKey;

use super::{PlannedDependency, SlotRef};
use crate::descriptors::{ComponentDescriptor, ComponentFactoryDescriptor, ProviderDescriptor};

/// The position of one component in a scope's slot layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct SlotIdx(usize);

impl SlotIdx {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// One planned component of a scope.
///
/// A slot without a factory is filled by a seed when the scope opens. `providers` are the
/// traits the component provides, in the order its built instance is aliased under them.
#[derive(Debug)]
pub(crate) struct SlotSpec {
    pub(crate) component: ComponentDescriptor,
    pub(crate) factory: Option<&'static ComponentFactoryDescriptor>,
    pub(crate) dependencies: Box<[PlannedDependency]>,
    pub(crate) providers: Box<[ProviderDescriptor]>,
}

impl SlotSpec {
    /// The index of `provider` among this slot's provider aliases.
    pub(crate) fn alias_of(&self, provider: &ProviderDescriptor) -> Option<usize> {
        self.providers.iter().position(|alias| {
            alias.trait_ty.type_id == provider.trait_ty.type_id
                && alias.qualifier == provider.qualifier
        })
    }
}

/// One provider alias reachable by its trait.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProviderSlot {
    pub(crate) at: SlotRef,
    pub(crate) descriptor: ProviderDescriptor,
}

/// The dense construction plan of one scope, shared by every instance of it opened in the
/// generation it was prepared for.
///
/// Slots are in one topological order, seeds first. Every eager dependency edge of every
/// factory is resolved to its source once, so construction never selects providers. The
/// type and trait indexes serve dynamic lookups and are built once with the layout.
#[derive(Debug)]
pub struct SlotLayout {
    pub(super) scope: ScopeKey,
    pub(super) slots: Box<[SlotSpec]>,
    pub(super) by_type: HashMap<TypeId, SlotIdx>,
    pub(super) by_trait: HashMap<TypeId, Box<[ProviderSlot]>>,
    pub(super) constructs: bool,
    pub(super) captures: bool,
}

impl SlotLayout {
    /// The scope this layout builds.
    pub fn scope(&self) -> ScopeKey {
        self.scope
    }

    /// The number of slots, seeds included.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the scope holds no components.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether opening the scope runs any factory. A scope without one holds only seeds,
    /// so an opening with no seeds needs no store.
    pub fn constructs(&self) -> bool {
        self.constructs
    }

    /// Whether a factory of this scope captures the scope under construction: a lazy,
    /// deferred, fresh or dynamic edge, or a transient built against its in-progress store.
    /// A scope that captures nothing is built without a shared store or resolver slot.
    pub(crate) fn captures(&self) -> bool {
        self.captures
    }

    /// The scope's components in construction order, seeds first.
    pub fn components(&self) -> impl Iterator<Item = &ComponentDescriptor> {
        self.slots.iter().map(|spec| &spec.component)
    }

    pub(crate) fn slots(&self) -> &[SlotSpec] {
        &self.slots
    }

    pub(crate) fn spec(&self, slot: SlotIdx) -> &SlotSpec {
        &self.slots[slot.index()]
    }

    pub(crate) fn slot_of(&self, type_id: TypeId) -> Option<SlotIdx> {
        self.by_type.get(&type_id).copied()
    }

    /// The provider aliases of `trait_id` in this scope, in provider order.
    pub(crate) fn providers_of(&self, trait_id: TypeId) -> &[ProviderSlot] {
        self.by_trait.get(&trait_id).map_or(&[], AsRef::as_ref)
    }
}
