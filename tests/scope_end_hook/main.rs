#![cfg(all(feature = "daemon", feature = "tracing-subscriber"))]

//! End-to-end tests for the observer-only `ScopeEnded` hook: instances ending through runs and
//! through opened handles, an observer that fails, and an observer that holds on.
//!
//! Every test tags its instances with a number of its own, so the shared record of ends can be
//! read back per test even when several tests share a process.

extern crate upwell as fork;

mod handles;
mod runs;
mod shutdown;
mod try_run;
mod typed;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tracing::Dispatch;
use tracing::dispatcher::DefaultGuard;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;
use upwell::daemon::App;
use upwell::logs::{ScopeLog, ScopeLogLayer, ScopeLogStream};
use upwell::scope::Root;
use upwell::{AnyScopeEnded, ScopeEnd, ScopeEnded, ScopeOpener, component, methods, scope};

/// The tag whose end the failing observer panics on.
const PANICKING: u64 = 900;

/// The tag whose end the gated observer waits on until [`RELEASE_RUN`] opens.
const GATED_RUN: u64 = 901;

/// The tag whose end the gated observer waits on until [`RELEASE_DRAIN`] opens.
const GATED_DRAIN: u64 = 902;

/// Every end the any-scope hook saw.
static ENDS: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// Every end the hook typed for [`Ended`] saw.
static TYPED_ENDED: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// Every end the hook typed for [`Sibling`] saw.
static TYPED_SIBLING: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// Counts [`DropProbe`]s that dropped.
static DROPS: AtomicUsize = AtomicUsize::new(0);

/// Opens when the run test lets its gated observer finish.
static RELEASE_RUN: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Opens when the drain test lets its gated observer finish.
static RELEASE_DRAIN: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The gate a tag's observer waits on, if the tag is gated.
fn gate(tag: u64) -> Option<&'static tokio::sync::Notify> {
    match tag {
        GATED_RUN => Some(&RELEASE_RUN),
        GATED_DRAIN => Some(&RELEASE_DRAIN),
        _ => None,
    }
}

/// An instance tagged by its test. The secret is not loggable.
#[scope(parents = [Root])]
pub struct Ended {
    #[log]
    tag: u64,
    #[allow(dead_code)]
    secret: String,
}

/// A component of [`Ended`] that counts its drops.
#[component(scope = Ended)]
struct DropProbe {}

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

/// A second instance kind, observed by a hook typed for it.
#[scope(parents = [Root])]
pub struct Sibling {
    #[log]
    tag: u64,
}

/// An entry into [`Ended`] that builds nothing.
#[upwell::entry(scope = Ended, key = "scope-end/nothing")]
async fn nothing() {}

/// An instance whose only component cannot be built.
#[scope(parents = [Root])]
pub struct Failing {
    #[log]
    tag: u64,
}

/// A component of [`Failing`] that cannot be built.
#[component(scope = Failing, default_factory = false)]
struct Broken;

#[methods]
impl Broken {
    #[init]
    fn new() -> Result<Self, std::io::Error> {
        Err(std::io::Error::other("the broken component cannot build"))
    }
}

/// Observes every ended instance.
#[component]
struct Recorder {}

#[methods]
impl Recorder {
    #[hook(AnyScopeEnded)]
    async fn ended(&self, end: ScopeEnd) {
        let tag = tag_of(&end);

        if let Some(gate) = gate(tag) {
            gate.notified().await;
        }

        tracing::info!("observed end");
        ENDS.lock().expect("the ends lock is healthy").push(end);
    }

    #[hook(ScopeEnded<Ended>)]
    async fn typed_ended(&self, end: ScopeEnd) {
        TYPED_ENDED
            .lock()
            .expect("the ends lock is healthy")
            .push(end);
    }

    #[hook(ScopeEnded<Sibling>)]
    async fn typed_sibling(&self, end: ScopeEnd) {
        TYPED_SIBLING
            .lock()
            .expect("the ends lock is healthy")
            .push(end);
    }

    #[hook(AnyScopeEnded)]
    async fn failing(&self, end: ScopeEnd) {
        assert_ne!(tag_of(&end), PANICKING, "the observer fails on purpose");
    }
}

/// The tag a recorded end carries as its loggable param, or zero for a scope without one.
fn tag_of(end: &ScopeEnd) -> u64 {
    end.params
        .iter()
        .find(|field| field.name == "tag")
        .and_then(|field| field.value.parse().ok())
        .unwrap_or(0)
}

fn ended(tag: u64) -> Ended {
    Ended {
        tag,
        secret: "hunter2".to_owned(),
    }
}

async fn app(name: &str) -> App {
    App::builder(name)
        .auto_discover()
        .build()
        .await
        .expect("the application builds")
}

fn opener(app: &App) -> ScopeOpener<Ended> {
    app.runtime()
        .opener::<Ended>()
        .expect("discovery declares the ended scope")
}

fn install() -> DefaultGuard {
    tracing::dispatcher::set_default(&Dispatch::new(
        Registry::default().with(ScopeLogLayer::new()),
    ))
}

async fn messages(stream: ScopeLogStream) -> Vec<String> {
    stream
        .filter_map(|item| async move {
            match item {
                ScopeLog::Record(record) => Some(record.message),
                _ => None,
            }
        })
        .collect()
        .await
}

/// The ends recorded for `tag` so far.
fn ends_of(tag: u64) -> Vec<ScopeEnd> {
    ENDS.lock()
        .expect("the ends lock is healthy")
        .iter()
        .filter(|end| tag_of(end) == tag)
        .cloned()
        .collect()
}

/// The end recorded for `tag`, awaited while the observers run on their own task.
async fn end_of(tag: u64) -> ScopeEnd {
    for _ in 0..2500 {
        if let Some(end) = ends_of(tag).into_iter().next() {
            return end;
        }

        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    panic!("no end was recorded for tag {tag}");
}
