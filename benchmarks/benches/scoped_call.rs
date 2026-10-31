//! One call through a connection-oriented protocol's per-call scope: an RPC call opening its
//! `Request` scope, and a WebSocket message opening its `WebsocketMessage` scope.
//!
//! Both protocols run the same cases over a real application with one open connection scope:
//!
//! - `dispatch_only`: an injection-free handler resolving through the connection scope, with no
//!   per-call scope. This is the baseline.
//! - `empty_scope`: the same handler in a fresh per-call scope with no components registered.
//! - `scoped`: a handler resolving one per-call component, the only one registered.
//! - `unused/N`: the injection-free handler in a fresh per-call scope with `N` registered
//!   components it never uses.
//! - `fan_in/8`: the injection-free handler in a fresh per-call scope whose components include one
//!   factory reading eight dependency edges, so the cost of reading planned edges is visible.
//!
//! Each case runs twice, once for wall-clock time (`rpc_call`, `ws_message`) and once for heap
//! bytes per call (`rpc_call_bytes`, `ws_message_bytes`). Per-call cost should track what a
//! handler uses, so a widening gap between `dispatch_only` and `unused/N` is a regression.

use criterion::{Criterion, criterion_group, criterion_main};
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::cases;
use upwell_benchmarks::measure::AllocBytes;
use upwell_benchmarks::rpc::RpcCalls;
use upwell_benchmarks::scoped;
use upwell_benchmarks::ws::WsMessages;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

fn call_time(c: &mut Criterion) {
    cases::time(c, "rpc_call", scoped::cases::<RpcCalls>);
    cases::time(c, "ws_message", scoped::cases::<WsMessages>);
}

fn call_bytes(c: &mut Criterion<AllocBytes>) {
    cases::bytes(c, "rpc_call_bytes", scoped::cases::<RpcCalls>);
    cases::bytes(c, "ws_message_bytes", scoped::cases::<WsMessages>);
}

criterion_group! {
    name = time;
    config = cases::time_config();
    targets = call_time
}

criterion_group! {
    name = bytes;
    config = cases::bytes_config();
    targets = call_bytes
}

criterion_main!(time, bytes);
