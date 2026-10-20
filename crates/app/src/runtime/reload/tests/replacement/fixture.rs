//! Fixture for the static state-aware replacement tests: a file-backed app with a
//! config-consuming [`Tally`] and a stateful [`Counter`] that depends on it and declares a
//! replacement factory.
//!
//! A config change reconstructs `Tally` (it injects `Cfg<CounterConfig>`), which forces
//! its fixed consumer `Counter` to be replaced. `Counter`'s replacement reads the
//! previous instance through [`Previous`] and carries its hit count forward. Components
//! use builder descriptors with unique IDs, so nothing registers into the link-time
//! inventory. The tests share [`REPLACEMENT`], so [`TEST_GUARD`] serializes them.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{Cfg, ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_di::{ComponentDescriptor, Previous, factories};

use crate::App;
use crate::test_support::test_component;

/// What `Counter`'s replacement factory does when the next transition invokes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Replacement {
    Carry,
    Fail,
    Panic,
}

static REPLACEMENT: std::sync::Mutex<Replacement> = std::sync::Mutex::new(Replacement::Carry);

static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) async fn lock_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_GUARD.lock().await
}

pub(super) fn set_replacement(replacement: Replacement) {
    *REPLACEMENT.lock().expect("replacement control lock") = replacement;
}

fn replacement() -> Replacement {
    *REPLACEMENT.lock().expect("replacement control lock")
}

#[derive(Deserialize)]
pub(super) struct CounterConfig {
    pub(super) label: String,
}

impl ConfigProperties for CounterConfig {
    const NAME: &'static str = "CounterConfig";
}

/// The config consumer that every label change reconstructs.
pub(super) struct Tally {
    pub(super) label: String,
}

test_component!(Tally, "replacement-tally");

/// A stateful fixed consumer of [`Tally`] whose hit count survives replacement.
pub(super) struct Counter {
    pub(super) hits: AtomicUsize,
    pub(super) tally: Arc<Tally>,
}

test_component!(Counter, "replacement-counter");

impl Counter {
    pub(super) fn hit(&self) -> usize {
        self.hits.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(super) fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn construct_tally(cfg: Cfg<CounterConfig>) -> Tally {
    Tally {
        label: cfg.snapshot().label.clone(),
    }
}

async fn construct_counter(tally: Arc<Tally>) -> Counter {
    Counter {
        hits: AtomicUsize::new(0),
        tally,
    }
}

async fn replace_counter(
    previous: Previous<Counter>,
    tally: Arc<Tally>,
) -> Result<Counter, &'static str> {
    match replacement() {
        Replacement::Carry => Ok(Counter {
            hits: AtomicUsize::new(previous.hits()),
            tally,
        }),
        Replacement::Fail => Err("the counter replacement is unavailable"),
        Replacement::Panic => panic!("the counter replacement panicked"),
    }
}

static TALLY_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Tally>()
    .with_factories(factories!["static" => construct_tally])
    .without_retention();

static COUNTER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Counter>()
    .with_factories(factories![
        "static" => construct_counter,
        replace "carry" => replace_counter,
    ]);

/// Builds the file-backed app with the counter config bound at `counter` and labelled
/// `label`. The temp dir is returned so the source file outlives the app.
pub(super) async fn build_counter_app(label: &str) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-replacement-")
        .tempdir()
        .expect("create temp dir");

    write_counter_config(&config_dir_of(&dir), label);

    let manager = ConfigManager::<Toml>::load_in_with_resolvers(
        &config_dir_of(&dir),
        &[],
        ResolverChain::empty(),
    )
    .expect("load config");
    let app = App::<()>::builder("replacement-test")
        .config_source(manager)
        .config::<CounterConfig>("counter")
        .component_descriptor(&TALLY_COMPONENT)
        .component_descriptor(&COUNTER_COMPONENT)
        .build()
        .await
        .expect("counter app builds");

    set_replacement(Replacement::Carry);

    (dir, app)
}

pub(super) use super::super::config_dir_of;

pub(super) fn write_counter_config(config_dir: &Path, label: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[counter]\nlabel = \"{label}\"\n"),
    )
    .expect("write counter config");
}
