//! End-to-end tests for RPC calls and connections across a runtime commit, over the in-memory
//! transport.
//!
//! [`Stamp`] is a singleton built from the config value of its runtime generation, so the
//! value a scoped component reads through it names the generation that component resolved on.
//! Rewriting the config file and reloading commits a new generation while connections and
//! calls are open.
#![cfg(feature = "daemon")]

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;
use tokio::sync::Notify;
use upwell::config::Toml;
use upwell::daemon::{App, Inject, handlers, service};
use upwell::{
    AppRuntime, CallResult, Cfg, ConfigManager, Lazy, MemoryConnectionHandle, component, config,
    methods,
};
use upwell_config::ResolverChain;

use common::{MemoryServer, deadline};

/// Fired when a parked call has resolved its request scope.
static PARKED: Notify = Notify::const_new();

/// Lets a parked call continue.
static RELEASE: Notify = Notify::const_new();

#[config(path = "stamp")]
#[derive(Deserialize)]
struct StampCfg {
    value: u32,
}

/// A singleton holding the config value its generation was built from.
#[component]
struct Stamp {
    #[default]
    value: u32,
}

#[methods]
impl Stamp {
    #[init]
    fn new(cfg: Cfg<StampCfg>) -> Self {
        Self {
            value: cfg.get().value,
        }
    }
}

/// A connection-scoped reader of [`Stamp`].
#[component(scope = upwell::daemon::Connection)]
struct ConnStamp {
    stamp: Arc<Stamp>,
}

/// A request-scoped reader of [`Stamp`].
#[component(scope = upwell::daemon::Request)]
struct CallStamp {
    stamp: Arc<Stamp>,
}

#[service(id = "reload", version = "0.1")]
struct ReloadSvc;

#[handlers]
impl ReloadSvc {
    /// Injects nothing, so a call to it only makes sure its connection is open.
    #[rpc]
    async fn ping() -> upwell::daemon::Result<(u32, u32)> {
        Ok((0, 0))
    }

    /// The stamps the call's request and connection scopes read.
    #[rpc]
    async fn stamps(
        Inject(call): Inject<Arc<CallStamp>>,
        Inject(conn): Inject<Arc<ConnStamp>>,
    ) -> upwell::daemon::Result<(u32, u32)> {
        Ok((call.stamp.value, conn.stamp.value))
    }

    /// Resolves its request stamp, parks until released, then builds the connection stamp.
    #[rpc]
    async fn parked(
        Inject(call): Inject<Arc<CallStamp>>,
        Inject(conn): Inject<Lazy<Arc<ConnStamp>>>,
    ) -> upwell::daemon::Result<(u32, u32)> {
        PARKED.notify_one();
        RELEASE.notified().await;

        let conn = conn
            .get_or_create()
            .await
            .expect("the connection stamp builds on demand");

        Ok((call.stamp.value, conn.stamp.value))
    }
}

/// A daemon over a config file holding `value`, served on a memory transport, with its runtime
/// and the config directory.
async fn start(value: u32) -> (MemoryServer, AppRuntime, TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-scopes-reload-")
        .tempdir()
        .expect("create temp config dir");

    write_value(dir.path(), value);

    let config =
        ConfigManager::<Toml>::load_in_with_resolvers(dir.path(), &[], ResolverChain::empty())
            .expect("load config");
    let daemon = App::builder("scopes-reload-test")
        .config_source(config)
        .auto_discover()
        .build()
        .await
        .expect("build daemon");
    let runtime = daemon.runtime().clone();

    (MemoryServer::start(daemon), runtime, dir)
}

fn write_value(dir: &Path, value: u32) {
    fs::write(
        dir.join("application.toml"),
        format!("[stamp]\nvalue = {value}\n"),
    )
    .expect("write config");
}

/// Rewrites the config to `value` and commits the generation built from it.
async fn commit(runtime: &AppRuntime, dir: &Path, value: u32) {
    write_value(dir, value);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(
        report.is_committed(),
        "a changed value commits a generation"
    );
}

/// Calls `method` and returns the (request, connection) stamps it read.
async fn call(conn: &MemoryConnectionHandle, method: &str) -> (u32, u32) {
    let payload = postcard::to_allocvec(&()).expect("unit encodes");

    match deadline(method, conn.call(method, payload))
        .await
        .expect("the call returns a response")
    {
        CallResult::Ok(body) => postcard::from_bytes(&body).expect("the stamps decode"),
        CallResult::Err { .. } => panic!("{method} errored"),
    }
}

#[tokio::test]
async fn a_call_in_flight_across_a_commit_resolves_on_its_own_generation() {
    let (server, runtime, dir) = start(1).await;
    let conn = server.connect().await;

    let (stamps, ()) = tokio::join!(call(&conn, "ReloadSvc.parked"), async {
        PARKED.notified().await;
        commit(&runtime, dir.path(), 2).await;
        RELEASE.notify_one();
    });

    assert_eq!(
        stamps,
        (1, 1),
        "a slot built on demand after the commit still comes from the call's generation"
    );

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn calls_on_a_connection_opened_before_a_commit_stay_on_its_generation() {
    let (server, runtime, dir) = start(1).await;
    let old = server.connect().await;
    let opened = call(&old, "ReloadSvc.ping").await;

    commit(&runtime, dir.path(), 2).await;

    let new = server.connect().await;
    let on_old = call(&old, "ReloadSvc.stamps").await;
    let on_new = call(&new, "ReloadSvc.stamps").await;

    assert_eq!(opened, (0, 0));
    assert_eq!(
        on_old,
        (1, 1),
        "calls, and connection slots built on demand, run on their connection's generation"
    );
    assert_eq!(
        on_new,
        (2, 2),
        "a new connection opens on the committed one"
    );

    server.shutdown([old, new]).await;
}
