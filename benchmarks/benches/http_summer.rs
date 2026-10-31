//! The framework comparison's summer-rs server: routes registered with summer-web's `#[get]` and
//! a singleton component read with the `Component` extractor.
//!
//! The app is built with `AppBuilder::build`, which runs every plugin without starting the
//! server, and its router is the one `WebPlugin` would serve, from `finalize_router`. summer has
//! no request scope, so `/scoped` builds its value by hand, as raw axum does. See
//! [`upwell_benchmarks::frameworks`] for the routes and groups shared with the other binaries.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use summer::App;
use summer::plugin::MutableComponentRegistry;
use summer_web::extractor::Component;
use summer_web::handler::auto_router;
use summer_web::{WebConfigurator, WebPlugin, finalize_router, get};
use tokio::runtime::Runtime;
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::cases;
use upwell_benchmarks::frameworks::{self, ALL_ROUTES, Server};
use upwell_benchmarks::measure::AllocBytes;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

/// Keeps summer's console logger off, so no case pays for log output.
const CONFIG: &str = "[logger]\nenable = false\n";

/// The shared singleton, registered as a summer component.
#[derive(Clone, Default)]
struct Greeter(Arc<AtomicU64>);

/// The per-request value, built by hand in the handler.
struct Ticket {
    id: u64,
}

#[get("/hello")]
async fn hello() -> &'static str {
    "hello"
}

#[get("/dep")]
async fn dep(Component(greeter): Component<Greeter>) -> String {
    format!("n={}", greeter.0.fetch_add(1, Ordering::Relaxed))
}

#[get("/scoped")]
async fn scoped() -> String {
    let ticket = Arc::new(Ticket { id: 0 });

    format!("t={}", ticket.id)
}

fn servers(runtime: &Runtime) -> Vec<Server> {
    let app = runtime.block_on(async {
        App::new()
            .use_config_str(CONFIG)
            .add_component(Greeter::default())
            .add_router(auto_router())
            .add_plugin(WebPlugin)
            .build()
            .await
            .expect("summer app builds")
    });

    vec![Server::new("summer", finalize_router(&app, ""), ALL_ROUTES)]
}

fn request_time(c: &mut Criterion) {
    frameworks::request_time(c, servers);
}

fn request_bytes(c: &mut Criterion<AllocBytes>) {
    frameworks::request_bytes(c, servers);
}

fn loopback(c: &mut Criterion) {
    frameworks::loopback(c, servers);
}

criterion_group! {
    name = time;
    config = cases::time_config();
    targets = request_time, loopback
}

criterion_group! {
    name = bytes;
    config = cases::bytes_config();
    targets = request_bytes
}

criterion_main!(time, bytes);
