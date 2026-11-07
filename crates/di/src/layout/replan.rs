//! What an entry plan depends on beyond its targets, so a later generation can tell whether
//! a plan still holds without planning it again.
//!
//! [`SlotLayout::plan_entry`] reads a layout's slot indexes, provider aliases and the shape
//! of each slot's dependency edges, plus the provider selection of the types its targets and
//! its included slots' unplanned edges name. A plan made from one layout is the plan another
//! layout makes from the same targets when both layouts agree on that shape
//! ([`plans_entries_like`](SlotLayout::plans_entries_like)) and selection agrees on every type
//! the plan read ([`entry_reads`](SlotLayout::entry_reads)).

use std::any::TypeId;

use upwell_core::DependencyDescriptor;

use super::{DepRef, EntryPlan, PlannedDependency, SlotIdx, SlotLayout, SlotSpec};

impl SlotLayout {
    /// Whether `other` plans every entry as this layout does, wherever provider selection
    /// agrees on the types a plan reads.
    ///
    /// Both layouts must hold the same component types at the same slots, with the same
    /// eager set, provider aliases and dependency edges of the same shape. Edges into the root,
    /// ancestors and transients may differ, since no entry plan follows them.
    pub fn plans_entries_like(&self, other: &SlotLayout) -> bool {
        self.scope.id() == other.scope.id()
            && self.eager == other.eager
            && self.slots.len() == other.slots.len()
            && self
                .slots
                .iter()
                .zip(other.slots.iter())
                .all(|(left, right)| specs_plan_alike(left, right))
    }

    /// The types whose provider selection `plan`, planned from `targets` on this layout,
    /// read: every target other than a config binding, and every edge a slot the plan
    /// builds or declares resolves through its own handle. Sorted and deduplicated.
    pub fn entry_reads(&self, plan: &EntryPlan, targets: &[DependencyDescriptor]) -> Box<[TypeId]> {
        let mut reads: Vec<TypeId> = targets
            .iter()
            .filter(|target| !target.config)
            .map(|target| target.ty.type_id)
            .collect();

        for (index, spec) in self.slots.iter().enumerate() {
            if !reaches(plan, SlotIdx::new(index)) {
                continue;
            }

            reads.extend(
                spec.dependencies
                    .iter()
                    .filter(|dependency| {
                        matches!(dependency.source, DepRef::Unplanned | DepRef::Contextual)
                    })
                    .map(|dependency| dependency.target),
            );
        }

        reads.sort_unstable();
        reads.dedup();

        reads.into_boxed_slice()
    }
}

/// Whether `plan` builds or declares `slot`.
fn reaches(plan: &EntryPlan, slot: SlotIdx) -> bool {
    plan.declared()[slot.index()]
}

fn specs_plan_alike(left: &SlotSpec, right: &SlotSpec) -> bool {
    left.component.ty.type_id == right.component.ty.type_id
        && left.providers.len() == right.providers.len()
        && left
            .providers
            .iter()
            .zip(right.providers.iter())
            .all(|(left, right)| {
                left.trait_ty.type_id == right.trait_ty.type_id && left.qualifier == right.qualifier
            })
        && left.dependencies.len() == right.dependencies.len()
        && left
            .dependencies
            .iter()
            .zip(right.dependencies.iter())
            .all(|(left, right)| edges_plan_alike(left, right))
}

fn edges_plan_alike(left: &PlannedDependency, right: &PlannedDependency) -> bool {
    left.target == right.target
        && left.qualifier == right.qualifier
        && left.cardinality == right.cardinality
        && sources_plan_alike(&left.source, &right.source)
}

/// Whether two edge sources lead an entry plan to the same slots: the same local slot, the
/// same providers of a collection, both unplanned, or both outside the scope.
fn sources_plan_alike(left: &DepRef, right: &DepRef) -> bool {
    match (left, right) {
        (DepRef::Local(left), DepRef::Local(right)) => left == right,

        (DepRef::Many(left), DepRef::Many(right)) => {
            left.len() == right.len()
                && left.iter().zip(right.iter()).all(|(left, right)| {
                    left.qualifier == right.qualifier
                        && sources_plan_alike(&left.source, &right.source)
                })
        }

        (DepRef::Unplanned, DepRef::Unplanned) | (DepRef::Contextual, DepRef::Contextual) => true,

        (left, right) => !is_followed(left) && !is_followed(right),
    }
}

/// Whether an entry plan follows `source` into its own scope.
fn is_followed(source: &DepRef) -> bool {
    matches!(
        source,
        DepRef::Local(_) | DepRef::Many(_) | DepRef::Unplanned | DepRef::Contextual
    )
}
