use std::any::{Any, TypeId};
use std::{fmt, future::Future, pin::Pin, sync::Mutex};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use upwell_core::{DependencyDescriptor, EntryKey, ItemSource, ResolverCtx, TypeDescriptor};
use upwell_di::{Component, ScopeContainer};

use crate::extract::ErrorResponse;

/// The interaction pattern of an RPC method, following gRPC's four kinds.
///
/// Only `Unary` is served today. The streaming variants are the target of the
/// streaming plan (`specs/002-streaming-rpcs/plan.md`); they are not yet
/// produced by the `#[rpc]` macro nor handled by the runtime.
#[derive(Clone, Copy, Debug)]
pub enum OperationKind {
    /// One request, one response.
    Unary,
    /// One request, a stream of responses.
    ServerStream,
    /// A stream of requests, one response.
    ClientStream,
    /// A bidirectional stream of requests and responses.
    BidiStream,
}

/// Classifies how a parameter value is sourced during an RPC call.
#[derive(Clone, Copy, Debug)]
pub enum ParameterKind {
    Component,
    Payload,
    Context,
    Cancellation,
    Metadata,
}

/// Static metadata describing a single RPC parameter.
#[derive(Clone, Copy, Debug)]
pub struct ParameterDescriptor {
    pub name: &'static str,
    pub kind: ParameterKind,
    pub ty: TypeDescriptor,
}

/// Context passed to an RPC handler on invocation.
///
/// Carries the raw postcard-encoded payload bytes and the call's **request
/// scope** — a `ScopeContainer` layered request → connection → singleton, so
/// `component::<T>()` and the `Inject<H>` extractor resolve singleton-, connection-,
/// and request-scoped components uniformly through it. Handler extractors (e.g.
/// `Payload<T>`) deserialize from `payload`.
///
/// For streaming calls, `requests` holds the inbound item stream (taken once by
/// the `Streaming<T>` extractor) and `cancel` fires when the peer cancels the
/// call or the connection drops.
pub struct RpcCallContext {
    pub payload: Vec<u8>,
    pub(crate) scope: std::sync::Arc<ScopeContainer>,
    /// The remote peer, carried directly so the `Peer` extractor reaches it
    /// without a connection-scope container — letting an otherwise-empty
    /// connection scope be skipped.
    pub(crate) peer: upwell_transport::PeerInfo,
    /// `Mutex<Option<_>>` because extractors borrow `&ctx`; the `Streaming<T>`
    /// extractor takes the receiver out exactly once.
    pub(crate) requests: Mutex<Option<mpsc::Receiver<Vec<u8>>>>,
    pub cancel: CancellationToken,
}

/// Resolution flows through the call's request scope, so config (`Cfg<T>`) and any other
/// external resolver reach a handler the same way they reach a constructor.
impl ResolverCtx for RpcCallContext {
    fn resolver(&self, kind: TypeId) -> Option<&dyn Any> {
        self.scope.resolver(kind)
    }
}

impl RpcCallContext {
    /// Builds a call context over the call's request `scope`. `requests` is `Some`
    /// only for client/bidirectional streaming calls; `cancel` is the call's
    /// cancellation token.
    pub fn new(
        payload: Vec<u8>,
        peer: upwell_transport::PeerInfo,
        scope: std::sync::Arc<ScopeContainer>,
        requests: Option<mpsc::Receiver<Vec<u8>>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            payload,
            scope,
            peer,
            requests: Mutex::new(requests),
            cancel,
        }
    }

    /// The call's request scope, for resolving scoped components.
    pub(crate) fn scope(&self) -> &std::sync::Arc<ScopeContainer> {
        &self.scope
    }

    /// The remote peer for this call. Public so guards and middleware can make
    /// admit/reject decisions on the peer without a handler extractor.
    pub fn peer(&self) -> &upwell_transport::PeerInfo {
        &self.peer
    }

    /// Resolves the component of type `T` (e.g. the stateful service singleton) as
    /// its handle (`Arc<T>` by default), through the request → connection →
    /// singleton chain. `None` if no such component is registered.
    pub fn component<T: Component>(&self) -> Option<T::Handle> {
        self.scope.get::<T>()
    }

    /// Takes the inbound request stream, if this is a streaming-input call and
    /// it has not already been taken.
    pub(crate) fn take_requests(&self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.requests.lock().expect("requests lock poisoned").take()
    }
}

/// The response returned by an RPC handler.
///
/// `payload` holds postcard-encoded response bytes. An empty payload is valid
/// for commands that return no meaningful data.
#[derive(Default)]
pub struct RpcResponse {
    pub payload: Vec<u8>,
}

/// The runtime result of an RPC handler: either a single unary response or a
/// stream of serialized response items. The serve loop drives whichever the
/// handler produced into the matching transport responder/sink; the handler's
/// declared `OperationKind` is metadata and does not select this at runtime.
pub enum RpcOutcome {
    Unary(RpcResponse),
    Stream(Pin<Box<dyn Stream<Item = core::result::Result<Vec<u8>, ErrorResponse>> + Send>>),
}

/// Async function pointer type for dispatching an RPC call.
pub type RpcHandler = fn(
    RpcCallContext,
) -> Pin<
    Box<dyn Future<Output = core::result::Result<RpcOutcome, ErrorResponse>> + Send>,
>;

/// Static metadata describing a single RPC method on a service.
pub struct RpcDescriptor {
    pub name: &'static str,
    pub operation: OperationKind,
    pub parameters: &'static [ParameterDescriptor],
    pub output: TypeDescriptor,
    pub handler: RpcHandler,
    /// The handler method's declaration, when the registering macro recorded it.
    pub source: Option<ItemSource>,
    /// The method's stable entry identity: `rpc/<service ID>.<method>`.
    pub key: EntryKey,
    /// The dependency edges of the method's injected parameters, in parameter order.
    pub targets: fn() -> Vec<DependencyDescriptor>,
}

impl fmt::Debug for RpcDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcDescriptor")
            .field("name", &self.name)
            .field("operation", &self.operation)
            .field("parameters", &self.parameters)
            .field("output", &self.output)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
