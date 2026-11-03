//! What kind of framework entry point an invocation context belongs to.

use std::sync::Arc;

use upwell_core::ScopeId;

/// The framework entry point that established an invocation context, with the static
/// metadata it knows when it establishes it.
///
/// Request data (headers, the matched route, the RPC method path) is not copied in: it stays
/// with the protocol's dispatch, so establishing a context never allocates for it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// One HTTP request served by the protocol's router, middleware included.
    HttpRequest,
    /// One WebSocket connection, between its upgrade and its close.
    WebsocketConnection,
    /// One message dispatched on a WebSocket connection.
    WebsocketMessage,
    /// One RPC call, middleware, handler and response stream included.
    RpcCall,
    /// One run of a scheduled job.
    JobRun {
        /// The job's name.
        job: Arc<str>,
    },
    /// Work run in a fresh instance of a scope by `ScopeBuilder::run`.
    ScopeRun {
        /// The scope the work runs in.
        scope: ScopeId,
    },
    /// One firing of a lifecycle hook kind.
    Hook {
        /// The hook kind's name, such as `startup`.
        kind: &'static str,
    },
}
