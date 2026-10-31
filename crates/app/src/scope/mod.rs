//! Immutable protocol-owned scope topology declarations and validation.

mod declaration;
mod plan;
mod prepared;
mod validate;

pub use declaration::{ScopeBoundary, ScopeParent, ScopeTopology};
pub(crate) use plan::{ScopeLayouts, ScopePlan, SeedDestination};
pub use prepared::PreparedScopeTopology;
pub use validate::ScopeTopologyError;

#[cfg(test)]
mod tests;
