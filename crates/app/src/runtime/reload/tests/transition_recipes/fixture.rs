//! Fixture for the transition-handler and hot-update tests: a file-backed app with a
//! config-consuming [`Tally`], a [`Greeter`] that depends on it and declares a transition
//! handler, and a config-consuming [`Banner`] that declares a hot update plus a vetoing
//! `ConfigReload` hook.
//!
//! A label change reconstructs `Tally`, which replaces its fixed consumer `Greeter`, and
//! invalidates `Banner`. Components use builder descriptors with unique IDs, so nothing
//! registers into the link-time inventory. The tests share the controls, so
//! [`TEST_GUARD`] serializes them.

use std::any::Any;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{
    Cfg, CfgNext, ConfigManager, ConfigProperties, ConfigReload, HookOutcome, ResolverChain, Toml,
};
use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor};
use upwell_di::{
    Component, ComponentChange, ComponentDescriptor, HotUpdate, OneShot, Previous, ReasonKind,
    factories,
};
use upwell_hooks::{HookDescriptor, HookParam};

use crate::App;
use crate::test_support::{HookFuture, hook_receiver, test_component};

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

static ONE_SHOT: Mutex<OneShotMode> = Mutex::new(OneShotMode::Decline);
static HOT_UPDATE: Mutex<HotUpdateMode> = Mutex::new(HotUpdateMode::Apply);
static VETO: AtomicBool = AtomicBool::new(false);
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

/// Makes `Banner`'s config-reload hook reject every proposal.
pub(super) fn set_veto(veto: bool) {
    VETO.store(veto, Ordering::SeqCst);
}

pub(super) fn banner_constructions() -> usize {
    BANNER_CONSTRUCTIONS.load(Ordering::SeqCst)
}

/// The label the candidate `Banner` held whenever the vetoing hook ran.
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

/// The config consumer that every label change reconstructs.
pub(super) struct Tally {
    pub(super) label: String,
}

test_component!(Tally, "recipe-tally");

/// A fixed consumer of [`Tally`] recording which recipe built it.
pub(super) struct Greeter {
    pub(super) origin: String,
    pub(super) tally: Arc<Tally>,
}

test_component!(Greeter, "recipe-greeter");

/// A registered component the greeter's ordinary factory does not declare.
pub(super) struct Spare;

test_component!(Spare, "recipe-spare");

/// A config consumer that updates its label in place.
pub(super) struct Banner {
    label: Mutex<String>,
}

test_component!(Banner, "recipe-banner");

impl Banner {
    pub(super) fn label(&self) -> String {
        self.label.lock().expect("banner label lock").clone()
    }
}

async fn construct_tally(cfg: Cfg<RecipeConfig>) -> Tally {
    Tally {
        label: cfg.snapshot().label.clone(),
    }
}

async fn construct_greeter(tally: Arc<Tally>) -> Greeter {
    Greeter {
        origin: "ordinary".to_owned(),
        tally,
    }
}

async fn construct_spare() -> Spare {
    Spare
}

/// Selects a one-shot factory for one transition per [`set_one_shot`].
fn pick_greeter(change: &ComponentChange) -> Option<OneShot<Greeter>> {
    let mode = std::mem::replace(
        &mut *ONE_SHOT.lock().expect("one-shot control lock"),
        OneShotMode::Decline,
    );
    let reason = if change.has_reason(ReasonKind::FixedDependent) {
        "fixed-dependent"
    } else {
        "other"
    };

    match mode {
        OneShotMode::Decline => None,
        OneShotMode::Fresh => Some(OneShot::new(move |tally: Arc<Tally>| async move {
            Greeter {
                origin: format!("one-shot:{reason}"),
                tally,
            }
        })),
        OneShotMode::Carry => Some(OneShot::replacing(
            move |previous: Previous<Greeter>, tally: Arc<Tally>| async move {
                Greeter {
                    origin: format!("one-shot after {}", previous.origin),
                    tally,
                }
            },
        )),
        OneShotMode::Undeclared => Some(OneShot::new(
            |tally: Arc<Tally>, _spare: Arc<Spare>| async move {
                Greeter {
                    origin: "undeclared".to_owned(),
                    tally,
                }
            },
        )),
        OneShotMode::Fail => Some(OneShot::new(|_tally: Arc<Tally>| async {
            Err::<Greeter, _>("the one-shot factory failed")
        })),
        OneShotMode::Panic => Some(OneShot::new(|_tally: Arc<Tally>| async {
            panic!("the one-shot factory panicked");

            #[allow(unreachable_code)]
            Err::<Greeter, &'static str>("unreachable")
        })),
        OneShotMode::Pending => Some(OneShot::new(|_tally: Arc<Tally>| async {
            std::future::pending::<Greeter>().await
        })),
    }
}

async fn construct_banner(cfg: Cfg<RecipeConfig>) -> Banner {
    BANNER_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

    Banner {
        label: Mutex::new(cfg.snapshot().label.clone()),
    }
}

/// Prepares `Banner`'s label update from the candidate config per [`set_hot_update`].
async fn update_banner(
    previous: Previous<Banner>,
    cfg: Cfg<RecipeConfig>,
) -> Result<HotUpdate<Banner>, String> {
    let label = cfg.snapshot().label.clone();
    let mode = *HOT_UPDATE.lock().expect("hot-update control lock");

    match mode {
        HotUpdateMode::Apply => Ok(HotUpdate::new(move |banner: &Banner| {
            *banner.label.lock().expect("banner label lock") = label;
        })),
        HotUpdateMode::Fail => Err(format!("cannot update '{}'", previous.label())),
        HotUpdateMode::Panic => panic!("the hot update panicked"),
        HotUpdateMode::PanicOnApply => Ok(HotUpdate::new(|_: &Banner| {
            panic!("the hot update panicked while applying")
        })),
    }
}

/// Records the retained banner's label, then rejects the proposal when vetoed.
fn banner_veto_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async move {
        let banner = hook_receiver::<Banner>(ctx)?;

        VETOED_LABELS
            .lock()
            .expect("veto record lock")
            .push(banner.label());

        if VETO.load(Ordering::SeqCst) {
            return Err(upwell_hooks::Error::Other("the banner vetoes".into()));
        }

        Ok(Box::new(HookOutcome::Unchanged) as Box<dyn Any + Send>)
    })
}

const BANNER_TY: TypeDescriptor = TypeDescriptor::of::<Banner>(Banner::NAME);

fn banner_veto_deps() -> Vec<DependencyDescriptor> {
    vec![<CfgNext<RecipeConfig> as HookParam<ConfigReload>>::dependency(None)]
}

static BANNER_HOOKS: [HookDescriptor; 1] =
    [
        HookDescriptor::of::<ConfigReload>(1, BANNER_TY, banner_veto_call)
            .with_dependencies(banner_veto_deps),
    ];

static TALLY_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Tally>()
    .with_factories(factories!["static" => construct_tally])
    .without_retention();

static GREETER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Greeter>()
    .with_factories(factories![
        "static" => construct_greeter,
        transition "pick" => pick_greeter,
    ]);

static SPARE_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<Spare>().with_factories(factories!["static" => construct_spare]);

static BANNER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Banner>()
    .with_factories(factories![
        "static" => construct_banner,
        hot_update "label" => update_banner,
    ])
    .with_hooks(|| &BANNER_HOOKS);

/// Builds the file-backed app labelled `label` with every control reset. The temp dir
/// is returned so the source file outlives the app.
pub(super) async fn build_recipe_app(label: &str) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-recipes-")
        .tempdir()
        .expect("create temp dir");

    write_recipe_config(&config_dir_of(&dir), label);
    set_one_shot(OneShotMode::Decline);
    set_hot_update(HotUpdateMode::Apply);
    set_veto(false);
    BANNER_CONSTRUCTIONS.store(0, Ordering::SeqCst);
    VETOED_LABELS.lock().expect("veto record lock").clear();

    let manager = ConfigManager::<Toml>::load_in_with_resolvers(
        &config_dir_of(&dir),
        &[],
        ResolverChain::empty(),
    )
    .expect("load config");
    let app = App::<()>::builder("transition-recipes-test")
        .config_source(manager)
        .config::<RecipeConfig>("recipe")
        .component_descriptor(&TALLY_COMPONENT)
        .component_descriptor(&GREETER_COMPONENT)
        .component_descriptor(&SPARE_COMPONENT)
        .component_descriptor(&BANNER_COMPONENT)
        .build()
        .await
        .expect("recipe app builds");

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
