//! Entry points: units of work that run inside a scope, registered once and prepared into a
//! dense table.
//!
//! Every entry point, whether a protocol route, an RPC method, a WebSocket handler, a plugin
//! entry or an entry into a user-defined scope, registers one [`EntryDescriptor`]. Preparation
//! validates each entry's scope and injected targets and indexes the entries by [`EntryId`],
//! so planning treats them all the same.

mod descriptor;
mod table;
mod validate;

pub use descriptor::{ENTRIES, EntryDescriptor, EntryOwner, EntryTargets};
pub(crate) use table::in_table_order;
pub use table::{EntryId, EntryTable};
pub use validate::EntryError;
pub(crate) use validate::validate_targets;

#[cfg(test)]
mod tests;
