//! End-to-end tests for RPC scopes that build only what a call uses, over the in-memory
//! transport.
//!
//! Each scoped component counts its builds, and every handler returns the counts, so a test
//! reads back which scopes built what: a connection builds its components the first time a
//! call injects them, and a call whose method plans nothing still opens its request scope
//! when a handler resolves from it.
#![cfg(feature = "daemon")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use upwell::daemon::{App, Inject, handlers, service};
use upwell::{CallResult, Lazy, MemoryConnectionHandle, component};

use common::{MemoryServer, deadline};

static CONNECTION_BUILDS: AtomicU64 = AtomicU64::new(0);
static REQUEST_BUILDS: AtomicU64 = AtomicU64::new(0);

/// Counts one [`ConnState`] build.
struct ConnBuild;

/// Counts one [`ReqState`] build.
struct ReqBuild;

impl Default for ConnBuild {
    fn default() -> Self {
        CONNECTION_BUILDS.fetch_add(1, Ordering::SeqCst);

        Self
    }
}

impl Default for ReqBuild {
    fn default() -> Self {
        REQUEST_BUILDS.fetch_add(1, Ordering::SeqCst);

        Self
    }
}

/// A connection-scoped component no method needs until `connection` is called.
#[component(scope = upwell::daemon::Connection)]
struct ConnState {
    #[default]
    _build: ConnBuild,
}

/// A request-scoped component only `lazy_request` reaches, through a lazy handle.
#[component(scope = upwell::daemon::Request)]
struct ReqState {
    #[default]
    _build: ReqBuild,
}

#[service(id = "on-demand", version = "0.1")]
struct OnDemandSvc;

#[handlers]
impl OnDemandSvc {
    /// Injects nothing scoped.
    #[rpc]
    async fn ping() -> upwell::daemon::Result<(u64, u64)> {
        Ok(builds())
    }

    /// Injects the connection-scoped component.
    #[rpc]
    async fn connection(
        Inject(_state): Inject<Arc<ConnState>>,
    ) -> upwell::daemon::Result<(u64, u64)> {
        Ok(builds())
    }

    /// Plans nothing, then builds the request-scoped component through its lazy handle.
    #[rpc]
    async fn lazy_request(
        Inject(state): Inject<Lazy<Arc<ReqState>>>,
    ) -> upwell::daemon::Result<(u64, u64)> {
        state
            .get_or_create()
            .await
            .expect("the request scope opens and builds its component");

        Ok(builds())
    }
}

/// The (connection, request) build counts so far.
fn builds() -> (u64, u64) {
    (
        CONNECTION_BUILDS.load(Ordering::SeqCst),
        REQUEST_BUILDS.load(Ordering::SeqCst),
    )
}

async fn start() -> MemoryServer {
    let daemon = App::builder("scopes-on-demand-test")
        .auto_discover()
        .build()
        .await
        .expect("build daemon");

    MemoryServer::start(daemon)
}

/// Calls `method` and returns the build counts it read.
async fn call(conn: &MemoryConnectionHandle, method: &str) -> (u64, u64) {
    let payload = postcard::to_allocvec(&()).expect("unit encodes");

    match deadline(method, conn.call(method, payload))
        .await
        .expect("the call returns a response")
    {
        CallResult::Ok(body) => postcard::from_bytes(&body).expect("the counts decode"),
        CallResult::Err { .. } => panic!("{method} errored"),
    }
}

#[tokio::test]
async fn a_connection_builds_its_components_when_a_call_first_injects_them() {
    let server = start().await;
    let conn = server.connect().await;

    let before = call(&conn, "OnDemandSvc.ping").await;
    let first = call(&conn, "OnDemandSvc.connection").await;
    let second = call(&conn, "OnDemandSvc.connection").await;

    assert_eq!(before, (0, 0), "opening a connection builds nothing");
    assert_eq!(first, (1, 0), "the first call injecting it builds it");
    assert_eq!(second, (1, 0), "later calls on the connection reuse it");

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn a_call_planning_nothing_opens_its_scope_when_its_handler_resolves_from_it() {
    let server = start().await;
    let conn = server.connect().await;

    let first = call(&conn, "OnDemandSvc.lazy_request").await;
    let second = call(&conn, "OnDemandSvc.lazy_request").await;

    assert_eq!(first, (0, 1));
    assert_eq!(second, (0, 2), "each call opens its own request scope");

    server.shutdown([conn]).await;
}
