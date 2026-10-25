//! The RPC protocol's per-call hot paths: wire-envelope (de)serialization and router dispatch.
//!
//! - `wire_envelope`: encoding and decoding a `WireMessage` (request and response) through
//!   `postcard` — the framing every call crosses, on top of the body codec measured in
//!   `serde_abstraction`.
//! - `router_dispatch`: `RpcRouter::dispatch` — the path→handler lookup plus handler invocation —
//!   across routing tables of 1/16/128 routes, to confirm dispatch cost is flat as a service grows.
//!   The handler is trivial, so the figure is routing overhead, not handler work.

use std::collections::HashMap;
use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use upwell_core::{EntryKey, ResolverSet, TypeDescriptor};
use upwell_di::{ScopeContainer, ScopeRegistry};
use upwell_rpc::descriptors::{RpcCallContext, RpcOutcome, RpcResponse};
use upwell_rpc::{
    ErrorResponse, OperationKind, ResolvedService, RpcDescriptor, RpcGroup, RpcRouter,
    ServiceDescriptor,
};
use upwell_transport::{PeerInfo, WireMessage, WireOutcome, WireRequest, WireResponse};

#[derive(Serialize, Deserialize)]
struct Args {
    account: u64,
    label: String,
    flags: Vec<u32>,
}

fn args() -> Args {
    Args {
        account: 99,
        label: "dispatch-benchmark".to_string(),
        flags: vec![1, 2, 4, 8],
    }
}

fn wire_envelope(c: &mut Criterion) {
    let body = postcard::to_allocvec(&args()).unwrap();

    let request = WireMessage::Request(WireRequest {
        id: 1,
        path: "BenchService.rpc0".to_string(),
        payload: body.clone(),
        streaming_input: false,
    });
    let response = WireMessage::Response(WireResponse {
        id: 1,
        outcome: WireOutcome::Ok(body),
    });

    let request_bytes = postcard::to_allocvec(&request).unwrap();
    let response_bytes = postcard::to_allocvec(&response).unwrap();

    let mut group = c.benchmark_group("rpc_wire_envelope");

    group.bench_function("request_encode", |bencher| {
        bencher.iter(|| black_box(postcard::to_allocvec(black_box(&request)).unwrap()));
    });

    group.bench_function("request_decode", |bencher| {
        bencher.iter(|| {
            black_box(postcard::from_bytes::<WireMessage>(black_box(&request_bytes)).unwrap())
        });
    });

    group.bench_function("response_encode", |bencher| {
        bencher.iter(|| black_box(postcard::to_allocvec(black_box(&response)).unwrap()));
    });

    group.bench_function("response_decode", |bencher| {
        bencher.iter(|| {
            black_box(postcard::from_bytes::<WireMessage>(black_box(&response_bytes)).unwrap())
        });
    });

    group.finish();
}

/// A type token for the bench service's `TypeDescriptor`.
struct BenchService;

/// A trivial handler: the dispatch bench measures routing, not handler work.
fn bench_handler(
    _ctx: RpcCallContext,
) -> Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>> {
    Box::pin(async { Ok(RpcOutcome::Unary(RpcResponse::default())) })
}

/// The service's group accessor — never invoked by `from_services` (which reads the flattened
/// `rpcs` vec), so an empty slice suffices.
fn empty_groups() -> &'static [RpcGroup] {
    &[]
}

/// Builds a router with `routes` distinct paths, all mapped to the trivial handler. Descriptors are
/// leaked to `'static`, which a benchmark can afford.
fn build_router(routes: usize) -> RpcRouter {
    let descriptors: Vec<&'static RpcDescriptor> = (0..routes)
        .map(|i| {
            let name: &'static str = Box::leak(format!("rpc{i}").into_boxed_str());
            let key: &'static str = Box::leak(format!("rpc/bench.{name}").into_boxed_str());

            &*Box::leak(Box::new(RpcDescriptor {
                name,
                operation: OperationKind::Unary,
                parameters: &[],
                output: TypeDescriptor::of::<()>("()"),
                handler: bench_handler,
                source: None,
                key: EntryKey::new(key).expect("bench RPC entry keys are valid"),
                targets: Vec::new,
            }))
        })
        .collect();

    let service = ResolvedService {
        descriptor: ServiceDescriptor {
            id: "bench",
            name: "BenchService",
            ty: TypeDescriptor::of::<BenchService>("BenchService"),
            version: None,
            rpcs: empty_groups,
        },
        rpcs: descriptors,
    };

    RpcRouter::from_services(std::slice::from_ref(&service))
}

async fn empty_scope() -> Arc<ScopeContainer> {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );

    ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), registry)
        .await
        .expect("root builds")
}

fn router_dispatch(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let scope = runtime.block_on(empty_scope());

    let mut group = c.benchmark_group("rpc_router_dispatch");

    for routes in [1usize, 16, 128] {
        let router = build_router(routes);

        group.bench_with_input(
            BenchmarkId::from_parameter(routes),
            &routes,
            |bencher, _| {
                bencher.to_async(&runtime).iter(|| async {
                    let ctx = RpcCallContext::new(
                        Vec::new(),
                        PeerInfo { addr: None },
                        Arc::clone(&scope),
                        None,
                        CancellationToken::new(),
                    );

                    black_box(router.dispatch("BenchService.rpc0", ctx).await)
                });
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .sample_size(100);
    targets = wire_envelope, router_dispatch
}
criterion_main!(benches);
