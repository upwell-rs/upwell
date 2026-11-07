//! Immutable protocol-owned scope topology declarations and validation.

mod declaration;
mod plan;
mod prepared;
mod registration;
mod validate;

pub use declaration::{ScopeBoundary, ScopeParent, ScopeTopology};
pub(crate) use plan::{
    EntryPlanning, Parallelism, PlanBase, ScopeLayouts, ScopePlan, SeedDestination,
};
pub use prepared::PreparedScopeTopology;
pub(crate) use registration::stores_params;
pub use registration::{SCOPES, ScopeDeclaration, rank_below};
pub use validate::ScopeTopologyError;

#[cfg(test)]
mod tests;
