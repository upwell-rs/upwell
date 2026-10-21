//! Fixture for the transition-strategy tests: a file-backed app in which one label change
//! exercises every component transition strategy.
//!
//! A label change reconstructs the config-consuming [`Tally`]. Its fixed consumers then
//! take their declared recipes: [`Greeter`] asks its transition handler and
//! [`Ledger`] uses its replacement factory. The live consumer [`Watcher`] keeps its
//! instance and is rebound in place, the config-consuming [`Banner`] hot-updates, and
//! [`Spare`] is untouched. `Banner` also owns a `ConfigReload` gate hook that runs once
//! every strategy has prepared.
//!
//! Components use builder descriptors with unique IDs, so nothing registers into the
//! link-time inventory. The tests share the controls, so [`TEST_GUARD`] serializes them.

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{ConfigManager, ConfigProperties, ResolverChain, Toml};

use crate::App;

mod components;

pub(super) use components::{
    AliveCounts, Banner, Greeter, Ledger, Spare, Tally, Watcher, alive_counts,
};

/// What `Greeter`'s transition handler selects on the next transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OneShotMode {
    Decline,
    Fresh,
    Carry,
    Undeclared,
    Fail,
    Panic,
    Pending,
}

/// What `Banner`'s hot-update preparation does on the next transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HotUpdateMode {
    Apply,
    Fail,
    Panic,
    PanicOnApply,
}

/// What `Banner`'s `ConfigReload` gate does with each proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GateMode {
    Accept,
    Veto,
    Park,
}

static ONE_SHOT: Mutex<OneShotMode> = Mutex::new(OneShotMode::Decline);
static HOT_UPDATE: Mutex<HotUpdateMode> = Mutex::new(HotUpdateMode::Apply);
static GATE: Mutex<GateMode> = Mutex::new(GateMode::Accept);
static PARKED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static TALLY_PANICS: AtomicBool = AtomicBool::new(false);
static BANNER_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);
static VETOED_LABELS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) async fn lock_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_GUARD.lock().await
}

/// Selects what the next transition's handler returns. The handler resets it to
/// [`OneShotMode::Decline`] once it selects, so later transitions fall back.
pub(super) fn set_one_shot(mode: OneShotMode) {
    *ONE_SHOT.lock().expect("one-shot control lock") = mode;
}

pub(super) fn set_hot_update(mode: HotUpdateMode) {
    *HOT_UPDATE.lock().expect("hot-update control lock") = mode;
}

/// Selects what `Banner`'s config-reload gate does with every later proposal.
pub(super) fn set_gate(mode: GateMode) {
    *GATE.lock().expect("gate control lock") = mode;
}

/// Makes every later ordinary `Tally` construction panic.
pub(super) fn set_tally_panics(panics: bool) {
    TALLY_PANICS.store(panics, Ordering::SeqCst);
}

/// Resolves once a gate in [`GateMode::Park`] has parked a proposal.
pub(super) async fn gate_parked() {
    PARKED.notified().await;
}

pub(super) fn banner_constructions() -> usize {
    BANNER_CONSTRUCTIONS.load(Ordering::SeqCst)
}

/// The label the candidate `Banner` held whenever the gate hook ran.
pub(super) fn vetoed_labels() -> Vec<String> {
    VETOED_LABELS.lock().expect("veto record lock").clone()
}

#[derive(Deserialize)]
pub(super) struct RecipeConfig {
    pub(super) label: String,
}

impl ConfigProperties for RecipeConfig {
    const NAME: &'static str = "RecipeConfig";
}

/// Builds the file-backed app labelled `label` with every control reset. The temp dir
/// is returned so the source file outlives the app.
pub(super) async fn build_recipe_app(label: &str) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-recipes-")
        .tempdir()
        .expect("create temp dir");
    let mut builder = App::<()>::builder("transition-recipes-test");

    write_recipe_config(&config_dir_of(&dir), label);
    set_one_shot(OneShotMode::Decline);
    set_hot_update(HotUpdateMode::Apply);
    set_gate(GateMode::Accept);
    set_tally_panics(false);
    BANNER_CONSTRUCTIONS.store(0, Ordering::SeqCst);
    VETOED_LABELS.lock().expect("veto record lock").clear();

    let manager = ConfigManager::<Toml>::load_in_with_resolvers(
        &config_dir_of(&dir),
        &[],
        ResolverChain::empty(),
    )
    .expect("load config");

    builder = builder
        .config_source(manager)
        .config::<RecipeConfig>("recipe");

    for component in components::COMPONENTS {
        builder = builder.component_descriptor(component);
    }

    let app = builder.build().await.expect("recipe app builds");

    (dir, app)
}

pub(super) use super::super::config_dir_of;

pub(super) fn write_recipe_config(config_dir: &Path, label: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[recipe]\nlabel = \"{label}\"\n"),
    )
    .expect("write recipe config");
}
