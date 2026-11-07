//! End-to-end tests for eager scoped components over the in-memory RPC transport.
//!
//! An eager component is built on every opening of its scope, whatever the call injects: an
//! eager connection component when the connection opens, and an eager request component on
//! every call, even one whose method injects nothing scoped. A component on the default
//! on-demand policy whose `#[init]` fails fails only the calls that use it.
#![cfg(feature = "daemon")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use upwell::daemon::{App, Inject, handlers, service};
use upwell::{CallResult, MemoryConnectionHandle, component, methods};

use common::{MemoryServer, deadline};

static CONNECTION_SPANS: AtomicU64 = AtomicU64::new(0);
static CALL_SPANS: AtomicU64 = AtomicU64::new(0);

/// Counts one [`ConnectionSpan`] build.
struct ConnectionBuild;

/// Counts one [`CallSpan`] build.
struct CallBuild;

impl Default for ConnectionBuild {
    fn default() -> Self {
        CONNECTION_SPANS.fetch_add(1, Ordering::SeqCst);

        Self
    }
}

impl Default for CallBuild {
    fn default() -> Self {
        CALL_SPANS.fetch_add(1, Ordering::SeqCst);

        Self
    }
}

/// An eager connection-scoped component, built when each connection opens.
#[component(scope = upwell::daemon::Connection, eager)]
struct ConnectionSpan {
    #[default]
    _build: ConnectionBuild,
}

/// An eager request-scoped component, built on every call.
#[component(scope = upwell::daemon::Request, eager)]
struct CallSpan {
    #[default]
    _build: CallBuild,
}

/// A request-scoped component whose constructor always fails.
#[component(scope = upwell::daemon::Request)]
struct Broken;

#[methods]
impl Broken {
    #[init]
    async fn init() -> Result<Self, std::io::Error> {
        Err(std::io::Error::other("the broken component cannot build"))
    }
}

/// A service whose methods report which scoped components their calls built.
#[service(id = "eager", version = "0.1")]
struct EagerSvc;

#[handlers]
impl EagerSvc {
    /// Injects nothing scoped.
    #[rpc]
    async fn ping() -> upwell::daemon::Result<(u64, u64)> {
        Ok(spans())
    }

    /// Injects the component that cannot build.
    #[rpc]
    async fn broken(Inject(_broken): Inject<Arc<Broken>>) -> upwell::daemon::Result<(u64, u64)> {
        Ok(spans())
    }
}

/// The (connection, call) span counts so far.
fn spans() -> (u64, u64) {
    (
        CONNECTION_SPANS.load(Ordering::SeqCst),
        CALL_SPANS.load(Ordering::SeqCst),
    )
}

async fn start() -> MemoryServer {
    let daemon = App::builder("scopes-eager-test")
        .auto_discover()
        .deny_unplanned()
        .build()
        .await
        .expect("build daemon");

    MemoryServer::start(daemon)
}

/// Calls `method` and returns the span counts it read, or `None` when the call failed.
async fn call(conn: &MemoryConnectionHandle, method: &str) -> Option<(u64, u64)> {
    let payload = postcard::to_allocvec(&()).expect("unit encodes");

    match deadline(method, conn.call(method, payload))
        .await
        .expect("the call returns a response")
    {
        CallResult::Ok(body) => Some(postcard::from_bytes(&body).expect("the counts decode")),
        CallResult::Err { .. } => None,
    }
}

#[tokio::test]
async fn eager_components_are_built_on_every_opening_of_their_scope() {
    let server = start().await;
    let conn = server.connect().await;

    let first = call(&conn, "EagerSvc.ping").await;
    let second = call(&conn, "EagerSvc.ping").await;

    assert_eq!(
        first,
        Some((1, 1)),
        "the connection and a call injecting nothing build their eager components"
    );
    assert_eq!(second, Some((1, 2)), "each call opens and builds its own");

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn a_failing_constructor_fails_only_the_calls_that_use_it() {
    let server = start().await;
    let conn = server.connect().await;

    let broken = call(&conn, "EagerSvc.broken").await;
    let ping = call(&conn, "EagerSvc.ping").await;

    assert_eq!(
        broken, None,
        "the call injecting the broken component fails"
    );
    assert!(ping.is_some(), "a call that does not use it still succeeds");

    server.shutdown([conn]).await;
}
