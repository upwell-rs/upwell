//! Stable entry-point identities.

crate::namespaced_id_type!(
    /// A stable namespaced identity of one entry point.
    ///
    /// An entry is a unit of work that runs inside a scope: an HTTP route, an RPC method,
    /// a WebSocket handler, a plugin entry, or an entry into a user-defined scope. Tooling
    /// and diagnostics key on this identity; the prepared application maps it to a dense
    /// process-local index.
    pub struct EntryKey,
    "entry"
);
