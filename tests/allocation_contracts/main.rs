#![cfg(feature = "ws")]

//! Deterministic allocation contracts for the per-call scopes of HTTP requests, WebSocket
//! messages and a custom scope opened through its [`ScopeOpener`](upwell_app::ScopeOpener).
//!
//! What a call allocates must follow what its entry plans, never the number of components
//! registered in its scope, and an HTTP route that injects nothing must allocate no more than
//! the same route on raw axum. RPC calls have the same contracts in `upwell-rpc`. Every call
//! runs on the test thread, to which allocation tracking is scoped.

#[path = "../../crates/di/src/test_support/alloc.rs"]
mod alloc;
mod http;
mod opener;
#[path = "../../crates/di/src/test_support/roster.rs"]
mod roster;
mod ws;

use std::sync::Arc;

use upwell_di::Component;

/// A singleton an entry injects, which a call reads without opening its scope.
struct Clock;

impl Component for Clock {
    type Handle = Arc<Self>;

    const ID: &'static str = "contract-clock";
    const NAME: &'static str = "Clock";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}
