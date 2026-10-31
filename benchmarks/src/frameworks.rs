//! A web-framework comparison harness: raw axum, summer-rs and Upwell serving the same routes.
//!
//! Each framework is its own bench binary (`http_raw`, `http_summer`, `http_upwell`), so one
//! framework's process-wide state, such as a global tracing dispatcher or link-time route
//! registry, never touches another's numbers. Every binary reports into the same Criterion
//! groups, so their cases compare side by side:
//!
//! - `frameworks_request`: wall-clock time of one request through the framework's router in
//!   process (`tower::ServiceExt::oneshot`), which isolates routing, extraction and DI cost.
//! - `frameworks_request_bytes`: heap bytes allocated by that request.
//! - `frameworks_loopback`: requests over loopback TCP to the router served by `axum::serve` on
//!   one dedicated server thread, driven by [`CONNECTIONS`] keep-alive HTTP/1 connections. The
//!   throughput is requests per second at a saturated server thread, like an `oha` run against a
//!   server pinned to one core.
//!
//! Every server serves [`HELLO`], and the full route set adds [`DEP`] and [`SCOPED`]:
//!
//! - [`HELLO`]: a static string, no dependencies.
//! - [`DEP`]: reads a shared singleton.
//! - [`SCOPED`]: uses one per-request value, built by hand where the framework has no request
//!   scope.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::http::{Request, StatusCode, header};
use criterion::{Criterion, Throughput};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;

use crate::cases::{self, Case};
use crate::http::HttpCall;
use crate::measure::AllocBytes;

/// The route serving a static string.
pub const HELLO: &str = "/hello";

/// The route reading a shared singleton.
pub const DEP: &str = "/dep";

/// The route using one per-request value.
pub const SCOPED: &str = "/scoped";

/// Every route of the comparison.
pub const ALL_ROUTES: &[&str] = &[HELLO, DEP, SCOPED];

/// The keep-alive connections the loopback group drives a server with.
pub const CONNECTIONS: usize = 64;

/// One framework configuration under test: its router and the routes it is measured on.
pub struct Server {
    name: String,
    router: Router,
    routes: &'static [&'static str],
}

impl Server {
    /// Measures `routes` of `router`, naming the cases after `name`.
    pub fn new(name: impl Into<String>, router: Router, routes: &'static [&'static str]) -> Self {
        Self {
            name: name.into(),
            router,
            routes,
        }
    }

    fn case_name(&self, route: &str) -> String {
        format!("{}/{}", self.name, route.trim_start_matches('/'))
    }
}

/// Measures the in-process request time of every route of every server `servers` builds.
pub fn request_time(c: &mut Criterion, servers: impl FnOnce(&Runtime) -> Vec<Server>) {
    cases::time(c, "frameworks_request", |runtime| {
        request_cases(servers(runtime))
    });
}

/// Measures the heap bytes one in-process request allocates, for every route of every server.
pub fn request_bytes(c: &mut Criterion<AllocBytes>, servers: impl FnOnce(&Runtime) -> Vec<Server>) {
    cases::bytes(c, "frameworks_request_bytes", |runtime| {
        request_cases(servers(runtime))
    });
}

fn request_cases(servers: Vec<Server>) -> Vec<Case<HttpCall>> {
    servers
        .iter()
        .flat_map(|server| {
            server.routes.iter().map(|route| {
                Case::new(
                    server.case_name(route),
                    HttpCall::new(server.router.clone(), route),
                )
            })
        })
        .collect()
}

/// Measures loopback TCP throughput for every route of every server `servers` builds.
///
/// # Panics
///
/// Panics when a server cannot be bound or reached, or a route does not answer `200 OK`.
pub fn loopback(c: &mut Criterion, servers: impl FnOnce(&Runtime) -> Vec<Server>) {
    let client = Runtime::new().expect("tokio runtime");
    let servers = servers(&client);
    let mut group = c.benchmark_group("frameworks_loopback");

    group
        .throughput(Throughput::Elements(1))
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5))
        .sample_size(20);

    for server in &servers {
        let running = Loopback::start(server.router.clone());
        let pool = client.block_on(Pool::connect(running.addr, CONNECTIONS));

        for route in server.routes {
            client.block_on(pool.run(route, CONNECTIONS as u64));

            group.bench_function(server.case_name(route), |b| {
                b.iter_custom(|iters| client.block_on(pool.run(route, iters)));
            });
        }

        drop(pool);
        running.stop();
    }

    group.finish();
}

/// A router served by `axum::serve` on its own single-threaded runtime and thread.
struct Loopback {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    thread: JoinHandle<()>,
}

impl Loopback {
    fn start(router: Router) -> Self {
        let (shutdown, stopped) = oneshot::channel();
        let (bound, addr) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");

            runtime.block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("loopback listener binds");

                bound
                    .send(listener.local_addr().expect("listener has an address"))
                    .expect("the bench waits for the address");

                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .expect("loopback server runs");
            });
        });
        let addr = addr.recv().expect("server thread reports its address");

        Self {
            addr,
            shutdown,
            thread,
        }
    }

    fn stop(self) {
        let _ = self.shutdown.send(());

        self.thread.join().expect("server thread exits");
    }
}

/// Keep-alive HTTP/1 connections to one server, each driven by its own task while measured.
struct Pool {
    senders: Mutex<Vec<SendRequest<Empty<Bytes>>>>,
}

impl Pool {
    async fn connect(addr: SocketAddr, connections: usize) -> Self {
        let mut senders = Vec::with_capacity(connections);

        for _ in 0..connections {
            let stream = TcpStream::connect(addr)
                .await
                .expect("loopback server accepts");
            let (sender, connection) = http1::handshake(TokioIo::new(stream))
                .await
                .expect("HTTP/1 handshake succeeds");

            tokio::spawn(connection);
            senders.push(sender);
        }

        Self {
            senders: Mutex::new(senders),
        }
    }

    /// Sends `requests` `GET route` requests spread over every connection, and returns the
    /// wall-clock time they took together.
    async fn run(&self, route: &'static str, requests: u64) -> Duration {
        let senders = std::mem::take(&mut *self.senders.lock().expect("pool lock"));
        let connections = senders.len() as u64;
        let start = Instant::now();
        let tasks: Vec<_> = senders
            .into_iter()
            .zip(0..)
            .map(|(mut sender, index)| {
                let count = requests / connections + u64::from(index < requests % connections);

                tokio::spawn(async move {
                    for _ in 0..count {
                        get(&mut sender, route).await;
                    }

                    sender
                })
            })
            .collect();
        let senders = futures::future::try_join_all(tasks)
            .await
            .expect("every connection task completes");
        let elapsed = start.elapsed();

        *self.senders.lock().expect("pool lock") = senders;

        elapsed
    }
}

async fn get(sender: &mut SendRequest<Empty<Bytes>>, route: &'static str) {
    let request = Request::get(route)
        .header(header::HOST, "127.0.0.1")
        .body(Empty::new())
        .expect("loopback request builds");

    sender.ready().await.expect("connection stays open");

    let response = sender.send_request(request).await.expect("server answers");

    assert_eq!(response.status(), StatusCode::OK, "{route} answers 200 OK");

    response
        .into_body()
        .collect()
        .await
        .expect("response body reads");
}
