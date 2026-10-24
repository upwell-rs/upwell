//! The recipe app's components: one per transition strategy a label change exercises.

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use upwell_config::{Cfg, CfgNext, ConfigReload, HookOutcome};
use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor};
use upwell_di::{
    Component, ComponentChange, ComponentDescriptor, Dep, HotUpdate, OneShot, Previous, ReasonKind,
    factories,
};
use upwell_hooks::{HookDescriptor, HookParam};

use super::{
    BANNER_CONSTRUCTIONS, GATE, GateMode, HOT_UPDATE, HotUpdateMode, ONE_SHOT, OneShotMode, PARKED,
    RecipeConfig, TALLY_PANICS, VETOED_LABELS,
};
use crate::test_support::{HookFuture, hook_receiver, test_component};

static TALLIES: AtomicUsize = AtomicUsize::new(0);
static GREETERS: AtomicUsize = AtomicUsize::new(0);
static LEDGERS: AtomicUsize = AtomicUsize::new(0);

/// Counts one live instance of its owner until dropped.
struct Alive(&'static AtomicUsize);

impl Alive {
    fn new(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);

        Self(counter)
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How many instances of each constructed-per-transition component are alive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in super::super) struct AliveCounts {
    pub(in super::super) tallies: usize,
    pub(in super::super) greeters: usize,
    pub(in super::super) ledgers: usize,
}

pub(in super::super) fn alive_counts() -> AliveCounts {
    AliveCounts {
        tallies: TALLIES.load(Ordering::SeqCst),
        greeters: GREETERS.load(Ordering::SeqCst),
        ledgers: LEDGERS.load(Ordering::SeqCst),
    }
}

/// The config consumer that every label change reconstructs. It opts out of retention,
/// yet [`Watcher`]'s live dependency on it still rebinds in place.
pub(in super::super) struct Tally {
    pub(in super::super) label: String,
    _alive: Alive,
}

test_component!(Tally, "recipe-tally");

/// A fixed consumer of [`Tally`] recording which recipe built it; it declares a
/// transition handler.
pub(in super::super) struct Greeter {
    pub(in super::super) origin: String,
    pub(in super::super) tally: Arc<Tally>,
    _alive: Alive,
}

test_component!(Greeter, "recipe-greeter");

impl Greeter {
    fn new(origin: String, tally: Arc<Tally>) -> Self {
        Self {
            origin,
            tally,
            _alive: Alive::new(&GREETERS),
        }
    }
}

/// A fixed consumer of [`Tally`] that declares a replacement factory, carrying its
/// entry count across replacements.
pub(in super::super) struct Ledger {
    pub(in super::super) entries: usize,
    pub(in super::super) tally: Arc<Tally>,
    _alive: Alive,
}

test_component!(Ledger, "recipe-ledger");

impl Ledger {
    fn new(entries: usize, tally: Arc<Tally>) -> Self {
        Self {
            entries,
            tally,
            _alive: Alive::new(&LEDGERS),
        }
    }
}

/// A live consumer of [`Tally`] that keeps its instance and is rebound in place.
pub(in super::super) struct Watcher {
    pub(in super::super) tally: Dep<Tally>,
}

test_component!(Watcher, "recipe-watcher");

/// A registered component the greeter's ordinary factory does not declare.
pub(in super::super) struct Spare;

test_component!(Spare, "recipe-spare");

/// A config consumer that updates its label in place.
pub(in super::super) struct Banner {
    label: Mutex<String>,
}

test_component!(Banner, "recipe-banner");

impl Banner {
    pub(in super::super) fn label(&self) -> String {
        self.label.lock().expect("banner label lock").clone()
    }
}

async fn construct_tally(cfg: Cfg<RecipeConfig>) -> Tally {
    assert!(
        !TALLY_PANICS.load(Ordering::SeqCst),
        "the tally construction panicked"
    );

    Tally {
        label: cfg.snapshot().label.clone(),
        _alive: Alive::new(&TALLIES),
    }
}

async fn construct_greeter(tally: Arc<Tally>) -> Greeter {
    Greeter::new("ordinary".to_owned(), tally)
}

async fn construct_ledger(tally: Arc<Tally>) -> Ledger {
    Ledger::new(0, tally)
}

async fn replace_ledger(previous: Previous<Ledger>, tally: Arc<Tally>) -> Ledger {
    Ledger::new(previous.entries + 1, tally)
}

async fn construct_watcher(tally: Dep<Tally>) -> Watcher {
    Watcher { tally }
}

async fn construct_spare() -> Spare {
    Spare
}

/// Selects a one-shot factory for one transition per
/// [`set_one_shot`](super::set_one_shot).
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
            Greeter::new(format!("one-shot:{reason}"), tally)
        })),
        OneShotMode::Carry => Some(OneShot::replacing(
            move |previous: Previous<Greeter>, tally: Arc<Tally>| async move {
                Greeter::new(format!("one-shot after {}", previous.origin), tally)
            },
        )),
        OneShotMode::Undeclared => Some(OneShot::new(
            |tally: Arc<Tally>, _spare: Arc<Spare>| async move {
                Greeter::new("undeclared".to_owned(), tally)
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

/// Prepares `Banner`'s label update from the candidate config per
/// [`set_hot_update`](super::set_hot_update).
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

/// Records the retained banner's label, then accepts, rejects or parks the proposal per
/// [`set_gate`](super::set_gate). It runs after candidate construction and hot-update
/// preparation, so every strategy has prepared by then.
fn banner_gate_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async move {
        let banner = hook_receiver::<Banner>(ctx)?;
        let mode = *GATE.lock().expect("gate control lock");

        VETOED_LABELS
            .lock()
            .expect("veto record lock")
            .push(banner.label());

        match mode {
            GateMode::Accept => {}
            GateMode::Veto => return Err(upwell_hooks::Error::Other("the banner vetoes".into())),
            GateMode::Park => {
                PARKED.notify_one();
                std::future::pending::<()>().await;
            }
        }

        Ok(Box::new(HookOutcome::Unchanged) as Box<dyn Any + Send>)
    })
}

const BANNER_TY: TypeDescriptor = TypeDescriptor::of::<Banner>(Banner::NAME);

fn banner_gate_deps() -> Vec<DependencyDescriptor> {
    vec![<CfgNext<RecipeConfig> as HookParam<ConfigReload>>::dependency(None)]
}

static BANNER_HOOKS: [HookDescriptor; 1] =
    [
        HookDescriptor::of::<ConfigReload>(1, BANNER_TY, banner_gate_call)
            .with_dependencies(banner_gate_deps),
    ];

static TALLY_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Tally>()
    .with_factories(factories!["static" => construct_tally])
    .without_retention();

static GREETER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Greeter>()
    .with_factories(factories![
        "static" => construct_greeter,
        transition "pick" => pick_greeter,
    ]);

static LEDGER_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<Ledger>().with_factories(factories![
        "static" => construct_ledger,
        replace "carry" => replace_ledger,
    ]);

static WATCHER_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<Watcher>().with_factories(factories!["static" => construct_watcher]);

static SPARE_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<Spare>().with_factories(factories!["static" => construct_spare]);

static BANNER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Banner>()
    .with_factories(factories![
        "static" => construct_banner,
        hot_update "label" => update_banner,
    ])
    .with_hooks(|| &BANNER_HOOKS);

/// Every recipe component, in registration order.
pub(super) static COMPONENTS: [&ComponentDescriptor; 6] = [
    &TALLY_COMPONENT,
    &GREETER_COMPONENT,
    &LEDGER_COMPONENT,
    &WATCHER_COMPONENT,
    &SPARE_COMPONENT,
    &BANNER_COMPONENT,
];
