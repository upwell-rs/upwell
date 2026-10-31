use std::any::TypeId;

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, ScopeId, ScopeKey};

use super::{DepRef, SlotIdx, SlotLayout};
use crate::descriptors::ComponentDescriptor;
use crate::registry::ProviderSelectionModel;
use crate::registry::scopes::scope_allows_with;

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
    /// A plan of `layout` that builds nothing, for a scope whose every slot is built on demand.
    pub fn nothing(layout: &SlotLayout) -> Self {
        Self {
            builds: vec![false; layout.len()].into_boxed_slice(),
        }
    }

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

/// The runtime provider selection slots are planned with: eager selection from the layout's
/// scope, over the scopes `can_access` lets a consumer reach.
pub(crate) struct EagerSelection<'a> {
    pub(crate) selection: &'a ProviderSelectionModel,
    pub(crate) can_access: &'a dyn Fn(ScopeKey, ScopeKey) -> bool,
}

impl SlotLayout {
    /// Plans the slots an entry injecting `targets` builds when it opens this scope.
    ///
    /// A target that a lazy or config handle resolves builds nothing up front. Every other
    /// target builds the slots it resolves to here, with their dependencies in this scope, as
    /// [`for_each_selected_slot`](Self::for_each_selected_slot) picks them. `can_reach` is the
    /// scope reachability `selection` was validated with.
    pub fn plan_entry(
        &self,
        targets: &[DependencyDescriptor],
        selection: &ProviderSelectionModel,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> EntryPlan {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach);
        let selector = EagerSelection {
            selection,
            can_access: &can_access,
        };
        let mut builds = vec![false; self.slots.len()].into_boxed_slice();

        for target in targets {
            if target.config || target.resolution != ResolutionMode::Eager {
                continue;
            }

            self.for_each_selected_slot(
                &selector,
                target.ty.type_id,
                target.qualifier,
                target.cardinality,
                |slot| self.include(&selector, slot, &mut builds),
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

    /// Calls `visit` with each slot of this scope an eager dependency on `target` resolves to:
    /// its concrete slot; the providers `selector` picks that live in this scope, one for a
    /// single value or the winner of each qualifier for a keyed map; or every provider of a
    /// collection.
    pub(crate) fn for_each_selected_slot(
        &self,
        selector: &EagerSelection<'_>,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
        mut visit: impl FnMut(SlotIdx),
    ) {
        if let Some(slot) = self.slot_of(target) {
            return visit(slot);
        }

        let selected = match cardinality {
            Cardinality::One => selector
                .selection
                .select_runtime_one(
                    target,
                    qualifier,
                    ResolutionMode::Eager,
                    self.scope,
                    &selector.can_access,
                )
                .into_iter()
                .collect(),
            Cardinality::Keyed => selector.selection.select_runtime_keyed(
                target,
                ResolutionMode::Eager,
                self.scope,
                &selector.can_access,
            ),
            Cardinality::Collection => return self.for_each_target_slot(target, visit),
        };

        for chosen in selected {
            if let Some(slot) = self.slot_of(chosen.provider.concrete_ty.type_id) {
                visit(slot);
            }
        }
    }

    /// Marks `slot` and every slot of this scope it depends on in `included`.
    ///
    /// An edge resolved through its own handle is followed to the slots `selector` picks for
    /// its target here, so a slot a factory reaches dynamically or hydrates later is built
    /// with it.
    pub(crate) fn include(
        &self,
        selector: &EagerSelection<'_>,
        slot: SlotIdx,
        included: &mut [bool],
    ) {
        if included[slot.index()] {
            return;
        }

        included[slot.index()] = true;

        for dependency in self.spec(slot).dependencies.iter() {
            match &dependency.source {
                DepRef::Unplanned => {
                    self.for_each_selected_slot(
                        selector,
                        dependency.target,
                        dependency.qualifier,
                        dependency.cardinality,
                        |target| self.include(selector, target, included),
                    );
                }

                source => self.include_source(selector, source, included),
            }
        }
    }

    fn include_source(
        &self,
        selector: &EagerSelection<'_>,
        source: &DepRef,
        included: &mut [bool],
    ) {
        match source {
            DepRef::Local(at) => self.include(selector, at.slot, included),

            DepRef::Many(provided) => {
                for entry in provided.iter() {
                    self.include_source(selector, &entry.source, included);
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
