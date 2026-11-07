//! The framework comparison's Upwell servers: a macro-built controller with a field-injected
//! singleton and a request-scoped component injected per route.
//!
//! Three applications are built from the same macro-generated components: `upwell`, plus
//! `upwell_unused10` and `upwell_unused64`, which also register 10 or 64 request-scoped
//! components no route uses. Components are registered explicitly rather than discovered, so each
//! application holds only its own. See [`upwell_benchmarks::frameworks`] for the routes and
//! groups shared with the other binaries.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use upwell::axum::prelude::*;
use upwell::prelude::*;
use upwell::{ComponentDescriptor, Descriptor};
use upwell_benchmarks::alloc::TrackingAllocator;
use upwell_benchmarks::cases;
use upwell_benchmarks::frameworks::{self, ALL_ROUTES, HELLO, SCOPED, Server};
use upwell_benchmarks::measure::AllocBytes;
use upwell_benchmarks::scoped;

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

/// The routes the applications with unused components are measured on.
const UNUSED_ROUTES: &[&str] = &[HELLO, SCOPED];

/// The shared singleton, field-injected into the controller.
#[component(by_value)]
#[derive(Clone)]
pub struct Greeter {
    #[default]
    n: Arc<AtomicU64>,
}

/// The per-request value: a request-scoped component built on routes that inject it.
#[component(scope = HttpRequest)]
pub struct Ticket {
    #[default]
    pub id: u64,
}

/// Declares request-scoped components no route uses, and [`UNUSED`] listing their descriptors.
macro_rules! unused_components {
    ($($name:ident),* $(,)?) => {
        $(
            /// A request-scoped component no route uses.
            #[component(scope = HttpRequest)]
            pub struct $name {
                #[default]
                pub v: u64,
            }
        )*

        /// Every unused component's descriptor, in declaration order.
        static UNUSED: &[ComponentDescriptor] = &[
            $( <$name as Descriptor<ComponentDescriptor>>::DESCRIPTOR ),*
        ];
    };
}

unused_components!(
    S0, S1, S2, S3, S4, S5, S6, S7, S8, S9, S10, S11, S12, S13, S14, S15, S16, S17, S18, S19, S20,
    S21, S22, S23, S24, S25, S26, S27, S28, S29, S30, S31, S32, S33, S34, S35, S36, S37, S38, S39,
    S40, S41, S42, S43, S44, S45, S46, S47, S48, S49, S50, S51, S52, S53, S54, S55, S56, S57, S58,
    S59, S60, S61, S62, S63,
);

#[controller(path = "/")]
pub struct Bench {
    greeter: Greeter,
}

#[handlers]
impl Bench {
    #[get("/hello")]
    async fn hello(&self) -> &'static str {
        "hello"
    }

    #[get("/dep")]
    async fn dep(&self) -> String {
        format!("n={}", self.greeter.n.fetch_add(1, Ordering::Relaxed))
    }

    #[get("/scoped")]
    async fn scoped(&self, Inject(ticket): Inject<Arc<Ticket>>) -> String {
        format!("t={}", ticket.id)
    }
}

/// An application serving [`Bench`] with the first `unused` of [`UNUSED`] registered too.
fn server(runtime: &Runtime, name: &str, unused: usize, routes: &'static [&'static str]) -> Server {
    let builder = scoped::builder::<Axum>("http-upwell-bench")
        .controller::<Bench>()
        .component::<Greeter>()
        .component::<Ticket>();
    let builder = UNUSED[..unused]
        .iter()
        .fold(builder, |builder, descriptor| {
            builder.component_descriptor(descriptor)
        });
    let app = runtime
        .block_on(builder.build())
        .expect("benchmark app builds");

    Server::new(name, app.protocol().router().clone(), routes)
}

fn servers(runtime: &Runtime) -> Vec<Server> {
    vec![
        server(runtime, "upwell", 0, ALL_ROUTES),
        server(runtime, "upwell_unused10", 10, UNUSED_ROUTES),
        server(runtime, "upwell_unused64", 64, UNUSED_ROUTES),
    ]
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
