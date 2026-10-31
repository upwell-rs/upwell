//! Prepared slot layouts: the dense construction plan of one scope, fixed when a runtime
//! generation is prepared.
//!
//! A layout lists every component the scope holds in one topological order and resolves
//! each eager dependency edge to a [`DepRef`] once, so opening the scope builds its slots
//! without selecting providers.

mod dep_ref;
mod entry_plan;
mod planner;
mod slot_layout;
#[cfg(test)]
mod tests;

pub(crate) use dep_ref::{
    DepRef, PlannedDependency, ProvidedRef, RootTarget, SlotRef, TransientPlan,
};
pub(crate) use entry_plan::EagerSelection;
pub use entry_plan::EntryPlan;
pub use slot_layout::SlotLayout;
pub(crate) use slot_layout::{ProviderSlot, SlotIdx, SlotSpec};
