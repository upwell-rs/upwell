pub use upwell_rpc::{
    App, AppBuilder, Cancel, Error, ErrorHandler, ErrorResponse, FallibleHandler, FromContext,
    Guard, GuardLayer, GuardService, Handler, Inject, OperationKind, ParameterDescriptor,
    ParameterKind, Payload, Peer, PreparedRpc, RequestStream, ResolvedService, Responder,
    ResponseError, ResponseStream, Result, RouterService, Rpc, RpcAppBuilder, RpcCallContext,
    RpcDescriptor, RpcGroup, RpcHandler, RpcLimits, RpcOutcome, RpcRequest, RpcResponse, RpcRouter,
    RpcRuntime, RpcService, RpcTarget, SERVICES, ServiceDescriptor, ServiceRpcs, Streaming,
    dispatch_fallible, dispatch_with,
};

/// Service/RPC route resolution, for introspecting the registered surface.
pub use upwell_rpc::routes::resolved_services;

/// The RPC component scopes.
pub use upwell_rpc::scope::{Connection, Request};

/// The RPC daemon macros, re-exported through `upwell-rpc` (which owns them). With the
/// facade's `daemon` feature, `upwell-rpc/facade` is on, so their generated code roots
/// protocol types at `::upwell::daemon::*` and core types at `::upwell::*`.
/// (`app!` is a protocol-agnostic core macro at the crate root, not here.)
pub use upwell_rpc::{handlers, rpc, service};

/// Re-exported so middleware authors can implement `tower::Layer` / `tower::Service`.
pub use upwell_rpc::tower;

/// Re-exported so `#[rpc(stream)]` client codegen can project a concrete stream's item
/// type. Hidden; referenced only by generated code.
#[doc(hidden)]
pub use upwell_rpc::__Stream;

/// The RPC byte-stream transport — the daemon's [`ProtocolTransport`](crate::client) impl
/// (`StreamClientTransport`), its response stream, and connect helpers. The agnostic client
/// surface (`Client` API, capability traits) lives at [`upwell::client`](crate::client);
/// this is the RPC carry that plugs into it. Gated behind the `client` feature.
#[cfg(feature = "client")]
pub use upwell_rpc::{RpcResponses, StreamClientTransport, connect_tcp};

#[cfg(all(feature = "client", unix))]
pub use upwell_rpc::connect_unix;

/// Deprecated alias for [`App`]. Renamed in 0.7.0; removed in 1.0.0.
#[deprecated(since = "0.7.0", note = "renamed to `App`; removed in 1.0.0")]
pub type Daemon = App;

/// Deprecated alias for [`AppBuilder`]. Renamed in 0.7.0; removed in 1.0.0.
#[deprecated(since = "0.7.0", note = "renamed to `AppBuilder`; removed in 1.0.0")]
pub type DaemonBuilder = AppBuilder;

/// Common imports for building an RPC daemon: `use upwell::daemon::prelude::*;` (pair
/// with the crate-root `use upwell::prelude::*;` for the core framework + `app!`).
pub mod prelude {
    pub use super::{
        App, FromContext, Handler, Inject, Payload, Peer, Responder, Rpc, RpcAppBuilder, Streaming,
        handlers, rpc, service,
    };

    pub use upwell_transport::TcpTransport;

    #[cfg(unix)]
    pub use upwell_transport::UnixTransport;
}
