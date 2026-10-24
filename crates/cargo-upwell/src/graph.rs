//! Deterministic, presentation-neutral graph queries over tooling documents.
//!
//! Upstream traversal moves from an observed resource toward what explains it. Downstream
//! traversal reverses that orientation, while conflicts remain symmetric.

mod error;
mod explain;
mod failure;
mod index;
mod model;
mod query;
mod relation;
mod selection;

pub use crate::TOOLING_SCHEMA_VERSION;
pub use error::{GraphEmitError, GraphQueryError, GraphSelectorKind};
pub use explain::explain_resource;
pub use failure::query_failure_graph;
pub use model::{
    CliArgumentOwnership, CliCommandOwnership, CliOwnershipSummary, ConditionStep, GraphDirection,
    GraphQuery, GraphRelationFamily, GraphSource, GraphView, ProviderSelection, ResolvedConsumer,
    ResourceExplanation,
};
pub use query::query_graph;

#[cfg(test)]
mod tests;
