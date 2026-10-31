use std::any::TypeId;

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, ScopeId};

use super::{DepRef, SlotIdx, SlotLayout};
use crate::descriptors::ComponentDescriptor;
use crate::registry::ProviderSelectionModel;
use crate::registry::scopes::scope_allows_with;
use crate::registry::selection::select_single_by;

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
    /// target builds the slots it resolves to here, with their dependencies in this scope: its
    /// concrete type, the provider eager runtime selection picks in `selection` for a single
    /// value (when that provider lives in this scope), or every provider of a collection.
    /// `can_reach` is the scope reachability `selection` was validated with.
    pub fn plan_entry(
        &self,
        targets: &[DependencyDescriptor],
        selection: &ProviderSelectionModel,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> EntryPlan {
        let mut builds = vec![false; self.slots.len()].into_boxed_slice();
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach);

        for target in targets {
            if target.config || target.resolution != ResolutionMode::Eager {
                continue;
            }

            let target_id = target.ty.type_id;

            if target.cardinality == Cardinality::One && self.slot_of(target_id).is_none() {
                let selected = selection.select_runtime_one(
                    target_id,
                    target.qualifier,
                    ResolutionMode::Eager,
                    self.scope,
                    &can_access,
                );

                if let Some(slot) =
                    selected.and_then(|chosen| self.slot_of(chosen.provider.concrete_ty.type_id))
                {
                    self.include(slot, &mut builds);
                }

                continue;
            }

            self.for_each_selected_slot(
                target.ty.type_id,
                target.qualifier,
                target.cardinality,
                |slot| self.include(slot, &mut builds),
            );
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

    /// Calls `visit` with each slot of this scope a dependency on `target` resolves to: its
    /// concrete slot; for one value, the provider `qualifier` names or the primary or sole
    /// provider, as resolution selects among this scope's providers; for a collection or keyed
    /// map, every provider.
    pub(crate) fn for_each_selected_slot(
        &self,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
        mut visit: impl FnMut(SlotIdx),
    ) {
        if cardinality != Cardinality::One {
            return self.for_each_target_slot(target, visit);
        }

        if let Some(slot) = self.slot_of(target) {
            return visit(slot);
        }

        let providers = self.providers_of(target);
        let chosen = match qualifier {
            Some(qualifier) => providers
                .iter()
                .find(|provider| provider.descriptor.qualifier == qualifier),
            None => select_single_by(providers, |provider| provider.descriptor.primary),
        };

        if let Some(provider) = chosen {
            visit(provider.at.slot);
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
                    self.for_each_selected_slot(
                        dependency.target,
                        dependency.qualifier,
                        dependency.cardinality,
                        |target| self.include(target, included),
                    );
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
