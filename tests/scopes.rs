//! End-to-end tests for scoped dependencies over the in-memory transport.
//!
//! Each scope's identity is a `#[default]` field seeded from an atomic counter, so
//! an instance's id is fixed for its lifetime: a connection-scoped instance keeps
//! one id across the calls on its connection, a request-scoped instance gets a
//! fresh id per call, and a transient gets a fresh id per resolution. The tests
//! assert exactly those relationships by reading the ids back through handlers.
#![cfg(feature = "daemon")]

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use upwell::daemon::{App, Inject, handlers, service};
use upwell::{CallResult, Dep, Lazy, MemoryConnectionHandle, PeerInfo, component, injectable};

use common::{MemoryServer, deadline};

static CONNECTION_IDS: AtomicU64 = AtomicU64::new(1);
static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);
static TRACE_IDS: AtomicU64 = AtomicU64::new(1);
static TRACED_IDS: AtomicU64 = AtomicU64::new(1);

/// Connection-scoped: one id per connection.
struct ConnId(u64);

impl Default for ConnId {
    fn default() -> Self {
        Self(CONNECTION_IDS.fetch_add(1, Ordering::Relaxed))
    }
}

/// Request-scoped: one id per call.
struct ReqId(u64);

impl Default for ReqId {
    fn default() -> Self {
        Self(REQUEST_IDS.fetch_add(1, Ordering::Relaxed))
    }
}

/// Transient: one id per resolution.
struct TraceId(u64);

impl Default for TraceId {
    fn default() -> Self {
        Self(TRACE_IDS.fetch_add(1, Ordering::Relaxed))
    }
}

/// Identity of a [`TracedState`]: one id per build, so a build that ran twice shows a gap.
struct TracedId(u64);

impl Default for TracedId {
    fn default() -> Self {
        Self(TRACED_IDS.fetch_add(1, Ordering::Relaxed))
    }
}

/// Connection-scoped component; depends on the framework-seeded peer.
#[component(scope = upwell::daemon::Connection)]
struct ConnState {
    _peer: PeerInfo,
    #[default]
    id: ConnId,
}

/// Request-scoped component; depends on the connection-scoped one.
#[component(scope = upwell::daemon::Request)]
struct ReqState {
    conn: Arc<ConnState>,
    #[default]
    id: ReqId,
}

/// Request-scoped component whose eager edges follow `#[default]` and lazy fields, so a
/// planned edge read by the wrong position would resolve the wrong dependency.
#[component(scope = upwell::daemon::Request)]
struct MixedState {
    #[default]
    _id: ReqId,
    trace: Lazy<Arc<Trace>>,
    req: Arc<ReqState>,
    markers: Vec<Arc<dyn ScopedMarker>>,
    conn: Arc<ConnState>,
}

/// Request-scoped component whose transient edge needs construction, so its factory gives up
/// the synchronous path after its `#[default]` field and builds through the async one.
#[component(scope = upwell::daemon::Request)]
struct TracedState {
    #[default]
    id: TracedId,
    req: Arc<ReqState>,
    trace: Arc<Trace>,
}

/// Transient component, rebuilt on each resolution.
#[component(scope = upwell::scope::Transient)]
struct Trace {
    #[default]
    id: TraceId,
}

#[injectable]
trait ScopedMarker: Send + Sync {
    fn name(&self) -> &'static str;
}

#[component(provide = dyn ScopedMarker, after = RequestMarker)]
struct RootMarker;

impl ScopedMarker for RootMarker {
    fn name(&self) -> &'static str {
        "root"
    }
}

#[component(scope = upwell::daemon::Request, provide = dyn ScopedMarker)]
struct RequestMarker;

impl ScopedMarker for RequestMarker {
    fn name(&self) -> &'static str {
        "request"
    }
}

#[service(id = "scopes", version = "0.1")]
struct ScopeSvc;

#[handlers]
impl ScopeSvc {
    /// Returns (connection id, request id) for this call.
    #[rpc]
    async fn ids(Inject(req): Inject<Arc<ReqState>>) -> upwell::daemon::Result<(u64, u64)> {
        Ok((req.conn.id.0, req.id.0))
    }

    /// Returns the ids of two transients resolved in one call.
    #[rpc]
    async fn two_traces(
        Inject(a): Inject<Arc<Trace>>,
        Inject(b): Inject<Arc<Trace>>,
    ) -> upwell::daemon::Result<(u64, u64)> {
        Ok((a.id.0, b.id.0))
    }

    /// Returns what a component with mixed field kinds resolved, positionally.
    #[rpc]
    async fn mixed(
        Inject(mixed): Inject<Arc<MixedState>>,
    ) -> upwell::daemon::Result<(u64, u64, Vec<String>, bool)> {
        let trace = mixed.trace.get_or_create().await.is_ok();
        let markers = mixed
            .markers
            .iter()
            .map(|marker| marker.name().to_string())
            .collect();

        Ok((mixed.conn.id.0, mixed.req.conn.id.0, markers, trace))
    }

    /// Returns the traced state's id and whether its edges resolved.
    #[rpc]
    async fn traced(
        Inject(traced): Inject<Arc<TracedState>>,
    ) -> upwell::daemon::Result<(u64, bool)> {
        let resolved = traced.req.conn.id.0 > 0 && traced.trace.id.0 > 0;

        Ok((traced.id.0, resolved))
    }

    /// Replaces the request state behind one live handle, and returns whether a second live
    /// handle onto the same component still reads the instance the request built.
    #[rpc]
    async fn live_handles(
        Inject(first): Inject<Dep<ReqState>>,
        Inject(second): Inject<Dep<ReqState>>,
    ) -> upwell::daemon::Result<bool> {
        let built = second.snapshot();

        first.replace(Arc::new(ReqState {
            conn: Arc::clone(&built.conn),
            id: ReqId(0),
        }));

        Ok(Arc::ptr_eq(&second.snapshot(), &built))
    }

    #[rpc]
    async fn ordered_markers(
        Inject(markers): Inject<Vec<Arc<dyn ScopedMarker>>>,
    ) -> upwell::daemon::Result<Vec<String>> {
        Ok(markers
            .iter()
            .map(|marker| marker.name().to_string())
            .collect())
    }
}

/// Builds the daemon, serves it on a memory transport, and returns the client so
/// the test can open several independent connections.
async fn start() -> MemoryServer {
    let daemon = App::builder("scopes-test")
        .auto_discover()
        .build()
        .await
        .expect("build daemon");

    MemoryServer::start(daemon)
}

fn enc<T: serde::Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).unwrap()
}

fn dec<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    postcard::from_bytes(bytes).unwrap()
}

async fn ids(conn: &MemoryConnectionHandle) -> (u64, u64) {
    match deadline("ScopeSvc.ids call", conn.call("ScopeSvc.ids", enc(&())))
        .await
        .expect("ids call returns a response")
    {
        CallResult::Ok(body) => dec::<(u64, u64)>(&body),
        CallResult::Err { .. } => panic!("ids call errored"),
    }
}

#[tokio::test]
async fn connection_scope_is_stable_within_a_connection() {
    let server = start().await;
    let conn = server.connect().await;

    let (c1, r1) = ids(&conn).await;
    let (c2, r2) = ids(&conn).await;

    assert_eq!(c1, c2, "connection-scoped id is stable across calls");
    assert_ne!(r1, r2, "request-scoped id is fresh per call");

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn connection_scope_differs_across_connections() {
    let server = start().await;

    let first = server.connect().await;
    let second = server.connect().await;

    let (c1, _) = ids(&first).await;
    let (c2, _) = ids(&second).await;

    assert_ne!(
        c1, c2,
        "each connection gets its own connection-scoped instance"
    );

    server.shutdown([first, second]).await;
}

#[tokio::test]
async fn transient_is_fresh_per_resolution() {
    let server = start().await;
    let conn = server.connect().await;

    let result = deadline(
        "ScopeSvc.two_traces call",
        conn.call("ScopeSvc.two_traces", enc(&())),
    )
    .await
    .expect("two_traces call returns a response");
    let (a, b) = match result {
        CallResult::Ok(body) => dec::<(u64, u64)>(&body),
        CallResult::Err { .. } => panic!("two_traces call errored"),
    };

    assert_ne!(a, b, "two transient resolutions yield distinct instances");

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn provider_order_is_global_across_visible_scopes() {
    let server = start().await;
    let conn = server.connect().await;
    let result = deadline(
        "ScopeSvc.ordered_markers call",
        conn.call("ScopeSvc.ordered_markers", enc(&())),
    )
    .await
    .expect("ordered_markers call returns a response");
    let names = match result {
        CallResult::Ok(body) => dec::<Vec<String>>(&body),
        CallResult::Err { .. } => panic!("ordered_markers call errored"),
    };

    assert_eq!(names, ["request", "root"]);

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn generated_factory_reads_each_edge_at_its_declared_position() {
    let server = start().await;
    let conn = server.connect().await;
    let result = deadline("ScopeSvc.mixed call", conn.call("ScopeSvc.mixed", enc(&())))
        .await
        .expect("mixed call returns a response");
    let (conn_id, req_conn_id, markers, trace) = match result {
        CallResult::Ok(body) => dec::<(u64, u64, Vec<String>, bool)>(&body),
        CallResult::Err { .. } => panic!("mixed call errored"),
    };

    assert_eq!(
        conn_id, req_conn_id,
        "the connection edge and the request state's connection are the same instance"
    );
    assert_eq!(
        markers,
        ["request", "root"],
        "the collection edge resolves every provider in order"
    );
    assert!(trace, "the lazy edge still resolves through its own handle");

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn a_factory_falling_back_to_its_async_path_builds_its_defaults_once() {
    let server = start().await;
    let conn = server.connect().await;
    let first = TRACED_IDS.load(Ordering::Relaxed);
    let result = deadline(
        "ScopeSvc.traced call",
        conn.call("ScopeSvc.traced", enc(&())),
    )
    .await
    .expect("traced call returns a response");
    let (id, resolved) = match result {
        CallResult::Ok(body) => dec::<(u64, bool)>(&body),
        CallResult::Err { .. } => panic!("traced call errored"),
    };

    assert!(resolved, "the planned and transient edges both resolve");
    assert_eq!(
        (id, TRACED_IDS.load(Ordering::Relaxed)),
        (first, first + 1),
        "the call built its traced state's default once, not once per factory path"
    );

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn live_handles_onto_a_request_component_do_not_share_a_cell() {
    let server = start().await;
    let conn = server.connect().await;
    let result = deadline(
        "ScopeSvc.live_handles call",
        conn.call("ScopeSvc.live_handles", enc(&())),
    )
    .await
    .expect("live_handles call returns a response");
    let unchanged = match result {
        CallResult::Ok(body) => dec::<bool>(&body),
        CallResult::Err { .. } => panic!("live_handles call errored"),
    };

    assert!(
        unchanged,
        "the generated factory stores a fixed handle, so each live read gets its own cell"
    );

    server.shutdown([conn]).await;
}
