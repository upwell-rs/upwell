//! The Upwell native RPC protocol, built on the protocol-agnostic `upwell-app` core.
//!
//! This crate provides the first-class [`Rpc`] protocol definition, router, `FromContext`
//! extractors, the tower middleware stack, the wire transports, and the serve loop on top
//! of [`upwell_app`]. Depend on it directly for a self-contained RPC framework
//! (`upwell` is always present for the core macros + vocabulary), or reach it through
//! the `upwell` facade's `daemon` feature.

#[cfg(feature = "client")]
pub mod client;
pub mod descriptors;
pub mod error;
pub mod extract;
pub mod middleware;
mod plugin;
pub mod protocol;
pub mod router;
pub mod routes;
pub mod scope;
mod target;

pub use error::{Error, Result};
pub use plugin::{PreparedRpc, Rpc, RpcAppBuilder};

pub use protocol::{RpcLimits, RpcRuntime};
pub use router::RpcRouter;
pub use routes::ResolvedService;
pub use target::RpcTarget;
/// The RPC daemon macros (`#[service]`, `#[handlers]`, `#[rpc]`), owned by this protocol crate.
/// Their generated code roots protocol types at this crate (`::upwell_rpc::*`) by default, or
/// at `::upwell::daemon::*` under the `facade` feature — so they work whether `upwell-rpc`
/// is used directly or through the `upwell` facade. The core macros (`app!`, `#[component]`,
/// …) come from `upwell` (the always-present core).
pub use upwell_rpc_macros::{handlers, rpc, service};

pub use descriptors::{
    Descriptor, OperationKind, ParameterDescriptor, ParameterKind, RpcCallContext, RpcDescriptor,
    RpcGroup, RpcHandler, RpcOutcome, RpcResponse, SERVICES, ServiceDescriptor, ServiceRpcs,
};
pub use extract::{
    Cancel, ErrorResponse, FallibleHandler, FromContext, Handler, Inject, Payload, Peer,
    RequestStream, Responder, ResponseError, ResponseStream, Streaming, dispatch_fallible,
    dispatch_with,
};
pub use middleware::{
    ErrorHandler, Guard, GuardLayer, GuardService, RouterService, RpcRequest, RpcService,
};

/// The RPC app type: an [`App`](upwell_app::App) specialized to [`Rpc`].
/// `App::builder(name)` resolves through this alias without a turbofish.
pub type App = upwell_app::App<Rpc>;

/// The RPC app builder: [`AppBuilder`](upwell_app::AppBuilder) specialized to [`Rpc`].
pub type AppBuilder = upwell_app::AppBuilder<Rpc>;

// Re-export the agnostic app surface so a standalone `upwell-rpc` user has one import.
pub use upwell_app::{
    AppRegistry, AppRuntime, LoggingConfig, Plugin, PreparedProtocol, ProtocolDefinition,
    ProtocolRuntime, Serve, ServerConfig, ShutdownHandle, ShutdownSignal,
};

/// Re-exported so macro-generated code can reach the `#[distributed_slice]` attribute for
/// the per-service / `SERVICES` slices through a stable path.
#[doc(hidden)]
pub use linkme;

/// Re-exported so macro-generated code can reach `inventory` (the alternate registration backend)
/// through a stable path, mirroring the `linkme` re-export.
#[doc(hidden)]
pub use inventory;

/// Re-exported so middleware authors can implement `tower::Layer` / `tower::Service`.
pub use tower;

/// The RPC byte-stream [`ProtocolTransport`](upwell_client::ProtocolTransport)
/// implementation and its connect helpers. The agnostic client surface (`Client`,
/// `ProtocolTransport`, …) lives in [`upwell_client`]; this is the RPC carry that plugs
/// into it. Gated behind the `client` feature.
#[cfg(feature = "client")]
pub use client::{RpcResponses, StreamClientTransport, connect_tcp};

#[cfg(all(feature = "client", unix))]
pub use client::connect_unix;

/// The transport substrate, re-exported for generated client code and custom transports.
pub mod transport {
    pub use upwell_transport::*;
}

/// Re-exported so a `#[rpc(stream)]` handler returning a concrete (un-introspectable) stream
/// type still yields a well-typed client.
#[doc(hidden)]
pub use futures::Stream as __Stream;

/// Re-exported so generated client traits can be annotated `#[async_trait]`.
#[cfg(feature = "client")]
#[doc(hidden)]
pub use async_trait;
