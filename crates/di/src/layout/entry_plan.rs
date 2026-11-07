use std::any::TypeId;
use std::sync::Arc;

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, ScopeId, ScopeKey};

use super::entry_params::EntryParam;
use super::positions::SlotPositions;
use super::slot_layout::is_eager;
use super::{DepRef, SlotIdx, SlotLayout};
use crate::descriptors::ComponentDescriptor;
use crate::registry::ProviderSelectionModel;
use crate::registry::scopes::scope_allows_with;

/// The slots one entry builds when it opens its scope: the layout's eager set, the slots its
/// targets resolve to, and every slot of the same scope those depend on.
///
/// Planned once per runtime generation from the entry's targets. A slot the plan leaves out
/// is built on demand, when something resolves it from the open scope. The plan also holds the
/// slots the entry declares: those it builds, those its lazy targets reach, and those the
/// invocation-context accessors reachable from it read. Under
/// [`UnplannedConstruction::Deny`] an on-demand build outside them is reported.
///
/// Its parameters pre-resolve the targets it injects to the sources they read, so extracting
/// a built target with [`ScopeContainer::extract_planned`](crate::ScopeContainer::extract_planned)
/// selects nothing and looks nothing up by type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryPlan {
    scope: ScopeKey,
    builds: Box<[bool]>,
    declared: Arc<[bool]>,
    accessed: bool,
    accessor_reads: Option<Arc<[TypeId]>>,
    unplanned: UnplannedConstruction,
    params: Box<[EntryParam]>,
    positions: Arc<SlotPositions>,
}

/// Whether a scope opened for an entry may build, on demand, a slot the entry does not
/// declare.
///
/// An entry declares the slots its plan builds, those its lazy targets reach and those the
/// invocation-context accessors reachable from it read. A scope
/// opened without an entry builds on demand under either policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnplannedConstruction {
    /// Undeclared slots are built on demand.
    #[default]
    Allow,
    /// Building an undeclared slot on demand fails, surfacing drift between the declared
    /// entries and what the application resolves.
    Deny,
}

impl EntryPlan {
    /// A plan of `layout` for a scope opened without an entry, whose slots are built on
    /// demand: it builds only the layout's eager set.
    pub fn on_demand(layout: &SlotLayout) -> Self {
        Self {
            scope: layout.scope(),
            builds: layout.eager().into(),
            declared: layout.eager().into(),
            accessed: false,
            accessor_reads: None,
            unplanned: UnplannedConstruction::Allow,
            params: Box::default(),
            positions: Arc::new(SlotPositions::of(layout, layout.eager())),
        }
    }

    /// Where a scope opened for this entry stores the slots it fills in place.
    pub(crate) fn positions(&self) -> &Arc<SlotPositions> {
        &self.positions
    }

    /// The scope this plan opens.
    pub(crate) fn scope(&self) -> ScopeKey {
        self.scope
    }

    /// The parameter for the target `target` injected with `qualifier` and `cardinality`.
    pub(crate) fn param(
        &self,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> Option<&EntryParam> {
        self.params
            .iter()
            .find(|param| param.is(target, qualifier, cardinality))
    }

    /// Whether opening the scope for this entry builds `slot`.
    pub(crate) fn builds(&self, slot: SlotIdx) -> bool {
        self.builds[slot.index()]
    }

    /// The slots this entry declares, when its scope may build no other slot on demand.
    pub(crate) fn declares(&self) -> Option<&Arc<[bool]>> {
        match self.unplanned {
            UnplannedConstruction::Allow => None,
            UnplannedConstruction::Deny => Some(&self.declared),
        }
    }

    /// The slots this entry builds or its lazy targets reach, under either policy.
    pub(crate) fn declared(&self) -> &[bool] {
        &self.declared
    }

    /// Whether a scope opened for this entry reads the seed of type `seed`: something the entry
    /// builds or its lazy targets reach depends on it, or the entry injects it. `layout` is the
    /// layout the plan was made from.
    pub fn reads_seed(&self, layout: &SlotLayout, seed: TypeId) -> bool {
        layout
            .slot_of(seed)
            .is_some_and(|slot| self.declared[slot.index()])
    }

    /// The seeds a scope opened for this entry reads, in `layout`'s construction order. `layout`
    /// is the layout the plan was made from.
    pub fn seeds_read<'a>(
        &'a self,
        layout: &'a SlotLayout,
    ) -> impl Iterator<Item = &'a ComponentDescriptor> {
        layout
            .seeds()
            .filter(|seed| self.reads_seed(layout, seed.ty.type_id))
    }

    /// The number of slots this entry builds.
    pub fn len(&self) -> usize {
        self.builds.iter().filter(|builds| **builds).count()
    }

    /// Whether this entry builds nothing in its scope.
    pub fn is_empty(&self) -> bool {
        !self.builds.contains(&true)
    }

    /// The types the walk for invocation-context edges consulted, sorted.
    pub(crate) fn accessor_reads(&self) -> &[TypeId] {
        self.accessor_reads.as_deref().unwrap_or_default()
    }

    /// Whether planning walked for invocation-context edges, which it does only when some
    /// component of the generation declares one. A plan made without the walk does not hold
    /// for a generation that declares such an edge.
    pub fn walked_accessors(&self) -> bool {
        self.accessor_reads.is_some()
    }

    /// Whether an invocation-context accessor the entry reaches resolves from a slot of this
    /// scope, so the scope must be open while the entry runs even when it builds nothing.
    pub fn is_accessed(&self) -> bool {
        self.accessed
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
    /// Plans the slots an entry injecting `targets` builds when it opens this scope: the
    /// layout's eager set, and what the targets need.
    ///
    /// A target that a lazy or config handle resolves builds nothing up front. Every other
    /// target builds the slots it resolves to here, with their dependencies in this scope, as
    /// [`for_each_selected_slot`](Self::for_each_selected_slot) picks them. `can_reach` is the
    /// scope reachability `selection` was validated with. The plan also declares the slots a
    /// lazy target resolves to, picked as for any other target, with their dependencies, and
    /// pre-resolves the source each target reads where it can.
    pub fn plan_entry(
        &self,
        targets: &[DependencyDescriptor],
        selection: &ProviderSelectionModel,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
        unplanned: UnplannedConstruction,
    ) -> EntryPlan {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach);
        let selector = EagerSelection {
            selection,
            can_access: &can_access,
        };
        let builds = self.built_up_front(&selector, targets);
        let accessed = self.accessed(&selector, targets);
        let declared = self.declared(&selector, targets, &builds, &accessed.slots);

        EntryPlan {
            scope: self.scope,
            declared,
            accessed: accessed.slots.contains(&true),
            accessor_reads: accessed.reads,
            unplanned,
            params: self.plan_params(targets, selection),
            positions: Arc::new(SlotPositions::of(self, &builds)),
            builds,
        }
    }

    /// The slots the targets of `targets` that are not lazy resolve to here, with their
    /// dependencies.
    fn built_up_front(
        &self,
        selector: &EagerSelection<'_>,
        targets: &[DependencyDescriptor],
    ) -> Box<[bool]> {
        let mut builds = self.eager.clone();

        for target in targets.iter().filter(|target| is_built_up_front(target)) {
            self.for_each_selected_slot(
                selector,
                target.ty.type_id,
                target.qualifier,
                target.cardinality,
                |slot| self.include(selector, slot, &mut builds),
            );
        }

        builds
    }

    /// The eager set: the slot of every eager component, with its dependencies here.
    /// `can_reach` is the scope reachability `selection` was validated with.
    pub(super) fn eager_closure(
        &self,
        selection: &ProviderSelectionModel,
        can_reach: &impl Fn(ScopeId, ScopeId) -> bool,
    ) -> Box<[bool]> {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, can_reach);
        let selector = EagerSelection {
            selection,
            can_access: &can_access,
        };
        let mut eager = vec![false; self.slots.len()].into_boxed_slice();

        for (index, spec) in self.slots.iter().enumerate() {
            if is_eager(spec) {
                self.include(&selector, SlotIdx::new(index), &mut eager);
            }
        }

        eager
    }

    /// The slots `builds` and `accessed` hold and those the lazy targets of `targets` resolve
    /// to here, with their dependencies.
    fn declared(
        &self,
        selector: &EagerSelection<'_>,
        targets: &[DependencyDescriptor],
        builds: &[bool],
        accessed: &[bool],
    ) -> Arc<[bool]> {
        let mut declares: Vec<bool> = builds
            .iter()
            .zip(accessed)
            .map(|(builds, accessed)| *builds || *accessed)
            .collect();

        for target in targets {
            if target.config || is_built_up_front(target) {
                continue;
            }

            self.for_each_selected_slot(
                selector,
                target.ty.type_id,
                target.qualifier,
                target.cardinality,
                |slot| self.include(selector, slot, &mut declares),
            );
        }

        declares.into()
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

                DepRef::Contextual => {}

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
            | DepRef::Unplanned
            | DepRef::Contextual => {}
        }
    }
}

/// Whether opening a scope for an entry builds what `target` resolves to.
fn is_built_up_front(target: &DependencyDescriptor) -> bool {
    !target.config && target.resolution == ResolutionMode::Eager
}
