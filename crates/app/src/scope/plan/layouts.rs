//! The slot layouts of one generation and the deduplicated plans of every entry.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, ScopeId,
};
use upwell_di::{EntryPlan, ProviderSelectionModel, SlotLayout, UnplannedConstruction};

use super::parallel::Parallelism;
use crate::entry::{EntryDescriptor, EntryId};
use crate::scope::PreparedScopeTopology;

/// The prepared slot layout of every protocol-owned scope, and the plan of every entry
/// opening one of them.
///
/// Entry plans are indexed like the application's [`EntryTable`](crate::EntryTable), whose
/// entries are fixed while each runtime generation plans them against its own layouts.
/// Entries with the same [`EntrySignature`] share one plan.
#[derive(Debug, Default)]
pub(crate) struct ScopeLayouts {
    by_scope: HashMap<ScopeId, Arc<SlotLayout>>,
    plans: Box<[Arc<EntryPlan>]>,
    by_signature: HashMap<EntrySignature, usize>,
    entries: Box<[Option<usize>]>,
    unplanned: UnplannedConstruction,
}

/// What one generation plans entries with: provider selection over the scope topology,
/// under one construction policy, split across threads as `parallelism` allows.
#[derive(Clone, Copy)]
pub(crate) struct EntryPlanning<'a> {
    pub(crate) selection: &'a ProviderSelectionModel,
    pub(crate) topology: &'a PreparedScopeTopology,
    pub(crate) unplanned: UnplannedConstruction,
    pub(crate) parallelism: Parallelism,
}

/// The generation a candidate is planned from: its layouts and plans, and the provider
/// selection they were planned with.
#[derive(Clone, Copy)]
pub(crate) struct PlanBase<'a> {
    pub(crate) layouts: &'a ScopeLayouts,
    pub(crate) selection: &'a ProviderSelectionModel,
}

/// What makes two entries plan alike within one generation: their scope and the set of
/// their targets, each with every attribute planning may read.
#[derive(Debug, Eq, Hash, PartialEq)]
struct EntrySignature {
    scope: ScopeId,
    targets: Box<[TargetSignature]>,
}

/// One target of an [`EntrySignature`]: its dependency descriptor without the parameter
/// name and the type's display name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct TargetSignature {
    type_id: TypeId,
    qualifier: Option<&'static str>,
    cardinality: Cardinality,
    resolution: ResolutionMode,
    observation: DependencyObservation,
    optional: bool,
    dynamic: bool,
    config: bool,
}

/// One unique signature still to plan or carry over: the layout it plans against, its
/// targets, and the base plan with the same signature, when one exists.
struct PendingPlan<'a> {
    layout: &'a SlotLayout,
    targets: Vec<DependencyDescriptor>,
    base: Option<&'a Arc<EntryPlan>>,
}

impl ScopeLayouts {
    /// Plans every entry of `entries` against the scope layouts `by_scope`, once per unique
    /// signature.
    ///
    /// A signature `base` planned under the same policy, against a layout that plans
    /// entries as its new one does, keeps its plan when selection resolves no type the plan
    /// read differently now. Every other signature is planned again.
    pub(crate) fn plan(
        by_scope: HashMap<ScopeId, Arc<SlotLayout>>,
        entries: &[EntryDescriptor],
        planning: &EntryPlanning<'_>,
        base: Option<PlanBase<'_>>,
    ) -> Self {
        let mut by_signature = HashMap::new();
        let mut pending = Vec::new();
        let mut indexes = Vec::with_capacity(entries.len());
        let base = base.filter(|base| base.layouts.unplanned == planning.unplanned);

        for entry in crate::entry::in_table_order(entries) {
            let scope = entry.scope.id();
            let index = by_scope.get(&scope).map(|layout| {
                let targets = (entry.targets)();
                let next = pending.len();
                let index = *by_signature
                    .entry(EntrySignature::new(scope, &targets))
                    .or_insert(next);

                if index == next {
                    pending.push(PendingPlan {
                        layout,
                        targets,
                        base: None,
                    });
                }

                index
            });

            indexes.push(index);
        }

        if let Some(base) = base {
            base.match_signatures(&by_signature, &mut pending, &by_scope);
        }

        let plans = Self::plan_pending(&pending, planning, base);

        Self {
            by_scope,
            plans,
            by_signature,
            entries: indexes.into_boxed_slice(),
            unplanned: planning.unplanned,
        }
    }

    /// The slot layout of `scope`.
    pub(crate) fn get(&self, scope: &ScopeId) -> Option<&Arc<SlotLayout>> {
        self.by_scope.get(scope)
    }

    /// Every scope's slot layout, in no particular order.
    #[cfg(feature = "tooling")]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ScopeId, &Arc<SlotLayout>)> {
        self.by_scope.iter()
    }

    /// The plan of the entry with dense identity `id`.
    pub(crate) fn entry(&self, id: EntryId) -> Option<&EntryPlan> {
        self.shared_entry(id).map(AsRef::as_ref)
    }

    /// The shared plan of the entry with dense identity `id`.
    pub(crate) fn shared_entry(&self, id: EntryId) -> Option<&Arc<EntryPlan>> {
        let index = (*self.entries.get(id.index())?)?;

        Some(&self.plans[index])
    }

    /// Carries over each pending plan whose base plan still holds and plans the rest, in
    /// signature order.
    fn plan_pending(
        pending: &[PendingPlan<'_>],
        planning: &EntryPlanning<'_>,
        base: Option<PlanBase<'_>>,
    ) -> Box<[Arc<EntryPlan>]> {
        let changed = base
            .map(|base| planning.selection.changed_selections(base.selection))
            .unwrap_or_default();

        planning
            .parallelism
            .map(pending, pending.len(), |pending| {
                match pending.carried(&changed) {
                    Some(carried) => {
                        debug_assert_eq!(
                            carried.as_ref(),
                            pending.fresh(planning).as_ref(),
                            "a carried-over entry plan must equal the plan made again"
                        );

                        Arc::clone(carried)
                    }

                    None => pending.fresh(planning),
                }
            })
            .into_boxed_slice()
    }
}

impl<'a> PlanBase<'a> {
    /// Points every pending signature at the base plan with the same signature, when the
    /// base planned its scope with a layout that plans entries as the new one does.
    fn match_signatures(
        &self,
        by_signature: &HashMap<EntrySignature, usize>,
        pending: &mut [PendingPlan<'a>],
        by_scope: &HashMap<ScopeId, Arc<SlotLayout>>,
    ) {
        let base = self.layouts;
        let mut alike = HashMap::<ScopeId, bool>::new();

        for (signature, index) in by_signature {
            let Some(planned) = base.by_signature.get(signature) else {
                continue;
            };
            let scope_alike = *alike.entry(signature.scope).or_insert_with(|| {
                base.by_scope
                    .get(&signature.scope)
                    .zip(by_scope.get(&signature.scope))
                    .is_some_and(|(before, after)| before.plans_entries_like(after))
            });

            if scope_alike {
                pending[*index].base = Some(&base.plans[*planned]);
            }
        }
    }
}

impl PendingPlan<'_> {
    /// The base plan this signature keeps, unless selection of a type it read changed.
    fn carried(&self, changed: &HashSet<TypeId>) -> Option<&Arc<EntryPlan>> {
        self.base.filter(|base| {
            !self
                .layout
                .entry_reads(base, &self.targets)
                .iter()
                .any(|read| changed.contains(read))
        })
    }

    /// Plans this signature against its layout.
    fn fresh(&self, planning: &EntryPlanning<'_>) -> Arc<EntryPlan> {
        Arc::new(self.layout.plan_entry(
            &self.targets,
            planning.selection,
            |consumer, dependency| planning.topology.is_reachable(&consumer, &dependency),
            planning.unplanned,
        ))
    }
}

impl EntrySignature {
    /// The signature of an entry into `scope` injecting `targets`, as a sorted set.
    fn new(scope: ScopeId, targets: &[DependencyDescriptor]) -> Self {
        let mut targets: Vec<TargetSignature> = targets.iter().map(TargetSignature::new).collect();

        targets.sort_unstable();
        targets.dedup();

        Self {
            scope,
            targets: targets.into_boxed_slice(),
        }
    }
}

impl TargetSignature {
    fn new(target: &DependencyDescriptor) -> Self {
        Self {
            type_id: target.ty.type_id,
            qualifier: target.qualifier,
            cardinality: target.cardinality,
            resolution: target.resolution,
            observation: target.observation,
            optional: target.optional,
            dynamic: target.dynamic,
            config: target.config,
        }
    }
}
