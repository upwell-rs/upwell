//! In-process RPC fixtures: routers built from hand-written descriptors, and a real RPC
//! application whose calls open their request scope as the serve loop does.
//!
//! [`RpcCalls`] skips the transport and middleware, so a bench measures the per-call scope and
//! router dispatch alone. Wire framing is measured separately by `rpc_dispatch`.

use std::future::Future;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;
use upwell_core::{EntryKey, Scope, TypeDescriptor};
use upwell_rpc::descriptors::{RpcCallContext, RpcHandler, RpcOutcome, RpcResponse};
use upwell_rpc::scope::{Connection, Request};
use upwell_rpc::{
    ErrorResponse, OperationKind, ResolvedService, Rpc, RpcDescriptor, RpcGroup, RpcRouter,
    ServiceDescriptor,
};
use upwell_transport::PeerInfo;

use crate::scoped::{self, CallScope, CallScoped, Roster, Route, ScopedCalls, ScopedProtocol};

/// The path of the handler that resolves nothing.
pub const HELLO: &str = "BenchService.hello";

/// The path of the handler that resolves [`CallScoped<0>`].
pub const SCOPED: &str = "BenchService.scoped";

/// A type token for the bench service's `TypeDescriptor`.
struct BenchService;

/// The service's group accessor. `from_services` reads the flattened `rpcs` list and never
/// calls it, so an empty slice suffices.
fn empty_groups() -> &'static [RpcGroup] {
    &[]
}

/// A unary RPC descriptor for `name` on the bench service, leaked to `'static`.
pub fn descriptor(name: &'static str, handler: RpcHandler) -> &'static RpcDescriptor {
    let key: &'static str = Box::leak(format!("rpc/bench.{name}").into_boxed_str());

    Box::leak(Box::new(RpcDescriptor {
        name,
        operation: OperationKind::Unary,
        parameters: &[],
        output: TypeDescriptor::of::<()>("()"),
        handler,
        source: None,
        key: EntryKey::new(key).expect("bench RPC entry keys are valid"),
        targets: Vec::new,
    }))
}

/// A router serving `rpcs` as methods of `BenchService`.
pub fn router(rpcs: Vec<&'static RpcDescriptor>) -> RpcRouter {
    let service = ResolvedService {
        descriptor: ServiceDescriptor {
            id: "bench",
            name: "BenchService",
            ty: TypeDescriptor::of::<BenchService>("BenchService"),
            version: None,
            rpcs: empty_groups,
        },
        rpcs,
    };

    RpcRouter::from_services(std::slice::from_ref(&service))
}

/// A trivial handler, so a bench measures routing and scope cost, not handler work.
pub fn hello(
    _ctx: RpcCallContext,
) -> Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>> {
    Box::pin(async { Ok(RpcOutcome::Unary(RpcResponse::default())) })
}

fn scoped(
    ctx: RpcCallContext,
) -> Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>> {
    let component = ctx.component::<CallScoped<0>>();

    Box::pin(async move {
        component.expect("the scoped case registers CallScoped<0> in the request scope");

        Ok(RpcOutcome::Unary(RpcResponse::default()))
    })
}

/// An RPC application whose calls open a request scope under one connection scope, then
/// dispatch through an [`RpcRouter`] serving [`HELLO`] and [`SCOPED`].
pub struct RpcCalls {
    calls: ScopedCalls<Rpc>,
    router: RpcRouter,
}

impl ScopedProtocol for RpcCalls {
    async fn build(roster: Roster) -> Self {
        let calls = ScopedCalls::build(
            scoped::builder("rpc-call-bench"),
            Connection::KEY,
            Request::KEY,
            roster,
        )
        .await;
        let router = router(vec![
            descriptor("hello", hello),
            descriptor("scoped", scoped),
        ]);

        Self { calls, router }
    }

    async fn call(&self, scope: CallScope, route: Route) -> bool {
        let path = match route {
            Route::Hello => HELLO,
            Route::Scoped => SCOPED,
        };
        let ctx = RpcCallContext::new(
            Vec::new(),
            PeerInfo { addr: None },
            self.calls.scope(scope).await,
            None,
            CancellationToken::new(),
        );

        self.router.dispatch(path, ctx).await.is_ok()
    }
}
