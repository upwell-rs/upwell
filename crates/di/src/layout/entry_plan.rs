use std::any::TypeId;

use upwell_core::{DependencyDescriptor, ResolutionMode};

use super::{DepRef, SlotIdx, SlotLayout};
use crate::descriptors::ComponentDescriptor;

/// The slots one entry builds when it opens its scope: the slots its targets resolve to and
/// every slot of the same scope those depend on.
///
/// Planned once per runtime generation from the entry's targets. A slot the plan leaves out
/// is built on demand, when something resolves it from the open scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryPlan {
    builds: Box<[bool]>,
}

impl EntryPlan {
    /// Whether opening the scope for this entry builds `slot`.
    pub(crate) fn builds(&self, slot: SlotIdx) -> bool {
        self.builds[slot.index()]
    }

    /// The number of slots this entry builds.
    pub fn len(&self) -> usize {
        self.builds.iter().filter(|builds| **builds).count()
    }

    /// Whether this entry builds nothing in its scope.
    pub fn is_empty(&self) -> bool {
        !self.builds.contains(&true)
    }

    /// The components this entry builds, in `layout`'s construction order. `layout` is the
    /// layout the plan was made from.
    pub fn components<'a>(
        &'a self,
        layout: &'a SlotLayout,
    ) -> impl Iterator<Item = &'a ComponentDescriptor> {
        layout
            .components()
            .zip(self.builds.iter())
            .filter_map(|(component, builds)| builds.then_some(component))
    }
}

impl SlotLayout {
    /// Plans the slots an entry injecting `targets` builds when it opens this scope.
    ///
    /// A target that a lazy or config handle resolves builds nothing up front. Every other
    /// target builds the slots it resolves to here, by concrete type or by every provider of
    /// its trait, together with their dependencies in this scope.
    pub fn plan_entry(&self, targets: &[DependencyDescriptor]) -> EntryPlan {
        let mut builds = vec![false; self.slots.len()].into_boxed_slice();

        for target in targets {
            if target.config || target.resolution != ResolutionMode::Eager {
                continue;
            }

            self.for_each_target_slot(target.ty.type_id, |slot| self.include(slot, &mut builds));
        }

        EntryPlan { builds }
    }

    /// Calls `visit` with each slot of this scope that holds `target`: its own slot, or
    /// every provider of the trait it names.
    pub(crate) fn for_each_target_slot(&self, target: TypeId, mut visit: impl FnMut(SlotIdx)) {
        match self.slot_of(target) {
            Some(slot) => visit(slot),
            None => {
                for provider in self.providers_of(target) {
                    visit(provider.at.slot);
                }
            }
        }
    }

    /// Marks `slot` and every slot of this scope it depends on in `included`.
    ///
    /// An edge resolved through its own handle is followed to the slots its target names
    /// here, so a slot a factory reaches dynamically or hydrates later is built with it.
    pub(crate) fn include(&self, slot: SlotIdx, included: &mut [bool]) {
        if included[slot.index()] {
            return;
        }

        included[slot.index()] = true;

        for dependency in self.spec(slot).dependencies.iter() {
            match &dependency.source {
                DepRef::Unplanned => {
                    self.for_each_target_slot(dependency.target, |target| {
                        self.include(target, included)
                    });
                }

                source => self.include_source(source, included),
            }
        }
    }

    fn include_source(&self, source: &DepRef, included: &mut [bool]) {
        match source {
            DepRef::Local(at) => self.include(at.slot, included),

            DepRef::Many(provided) => {
                for entry in provided.iter() {
                    self.include_source(&entry.source, included);
                }
            }

            DepRef::Root(_)
            | DepRef::Ancestor { .. }
            | DepRef::Transient(_)
            | DepRef::Absent
            | DepRef::Unplanned => {}
        }
    }
}
