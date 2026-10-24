//! Proof that a reload requested from inside a running transition of the same runtime —
//! from a config-reload hook or a candidate factory — fails fast with
//! `ReentrantReload` instead of deadlocking on the leases its own transaction holds,
//! while the outer reload keeps its normal outcome and releases every lease.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::dirs::{Config, DirectoriesManager};
use upwell::{
    App, AppError, Cfg, CfgNext, ConfigManager, ConfigReload, HookOutcome, RuntimeReloader,
    TransitionAttemptId, component, config, methods,
};
use upwell_config::ResolverChain;

/// Bound on any reload in this file: a deadlock regression fails instead of hanging.
const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);

#[config(path = "svc")]
#[derive(Deserialize)]
struct SvcCfg {
    value: u32,
}

/// What one nested reload request returned, reduced to its comparable shape.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Nested {
    Committed,
    Reentrant(TransitionAttemptId),
    Unavailable,
    Other(String),
}

impl Nested {
    fn of(result: Result<upwell::RuntimeTransitionReport, AppError>) -> Self {
        match result {
            Err(AppError::ReentrantReload { attempt }) => Self::Reentrant(attempt),
            Err(AppError::RuntimeUnavailable) => Self::Unavailable,
            Err(other) => Self::Other(other.to_string()),
            Ok(report) if report.is_committed() => Self::Committed,
            Ok(report) => Self::Other(format!("uncommitted reload: {report:?}")),
        }
    }
}

static HOOK_NESTED: Mutex<Vec<Nested>> = Mutex::new(Vec::new());
static FACTORY_NESTED: Mutex<Vec<Nested>> = Mutex::new(Vec::new());

fn recorded(log: &Mutex<Vec<Nested>>) -> Vec<Nested> {
    log.lock().expect("nested reload log is available").clone()
}

fn record(log: &Mutex<Vec<Nested>>, nested: Nested) {
    log.lock()
        .expect("nested reload log is available")
        .push(nested);
}

/// A config-reload hook that tries to reload its own application.
#[component]
struct ReloadingHook {
    #[config("svc")]
    svc: Cfg<SvcCfg>,
    reloader: RuntimeReloader,
}

#[methods]
impl ReloadingHook {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("svc")] _next: CfgNext<SvcCfg>,
    ) -> Result<HookOutcome, AppError> {
        record(&HOOK_NESTED, Nested::of(self.reloader.reload().await));

        Ok(HookOutcome::Reloaded)
    }
}

/// A candidate factory that tries to reload its own application while it is
/// reconstructed for a changed binding.
#[component(factory = ReloadingFactory::make)]
struct ReloadingFactory {
    svc: Cfg<SvcCfg>,
}

impl ReloadingFactory {
    async fn make(svc: Cfg<SvcCfg>, reloader: RuntimeReloader) -> Self {
        record(&FACTORY_NESTED, Nested::of(reloader.reload().await));

        Self { svc }
    }
}

#[config(path = "cycle_a")]
#[derive(Deserialize)]
struct CycleACfg {
    value: u32,
}

#[config(path = "cycle_b")]
#[derive(Deserialize)]
struct CycleBCfg {
    value: u32,
}

static CYCLE_A_RELOADER: OnceLock<RuntimeReloader> = OnceLock::new();
static CYCLE_B_RELOADER: OnceLock<RuntimeReloader> = OnceLock::new();
static CYCLE_A_NESTED: Mutex<Vec<Nested>> = Mutex::new(Vec::new());
static CYCLE_B_NESTED: Mutex<Vec<Nested>> = Mutex::new(Vec::new());

/// Runtime A's hook reloads runtime B inline, which is allowed: B's leases are free.
#[component]
struct CycleAHook {
    #[config("cycle_a")]
    cfg: Cfg<CycleACfg>,
}

#[methods]
impl CycleAHook {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("cycle_a")] _next: CfgNext<CycleACfg>,
    ) -> Result<HookOutcome, AppError> {
        let reloader = CYCLE_B_RELOADER.get().expect("runtime B is attached");

        record(&CYCLE_A_NESTED, Nested::of(reloader.reload().await));

        Ok(HookOutcome::Reloaded)
    }
}

/// Runtime B's hook reloads runtime A, closing an `A -> B -> A` cycle on one task.
#[component]
struct CycleBHook {
    #[config("cycle_b")]
    cfg: Cfg<CycleBCfg>,
}

#[methods]
impl CycleBHook {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("cycle_b")] _next: CfgNext<CycleBCfg>,
    ) -> Result<HookOutcome, AppError> {
        let reloader = CYCLE_A_RELOADER.get().expect("runtime A is attached");

        record(&CYCLE_B_NESTED, Nested::of(reloader.reload().await));

        Ok(HookOutcome::Reloaded)
    }
}

/// A temp config dir holding `application.toml` with the given `svc.value`.
fn config_dir(value: u32) -> (TempDir, PathBuf, ConfigManager<Toml>) {
    let root = tempfile::Builder::new()
        .prefix("upwell-reload-reentrancy-")
        .tempdir()
        .expect("create temp config dir");
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    write_value(&config_file, value);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    (root, config_file, manager)
}

fn write_value(config_file: &PathBuf, value: u32) {
    fs::write(
        config_file,
        format!(
            "[svc]\nvalue = {value}\n\n[cycle_a]\nvalue = {value}\n\n[cycle_b]\nvalue = {value}\n"
        ),
    )
    .expect("write config");
}

async fn reload_within_timeout(
    reloader: &RuntimeReloader,
) -> Result<upwell::RuntimeTransitionReport, AppError> {
    tokio::time::timeout(RELOAD_TIMEOUT, reloader.reload())
        .await
        .expect("the reload completes instead of deadlocking")
}

#[tokio::test]
async fn a_hook_reloading_its_own_runtime_gets_reentrant_reload() {
    let (_root, config_file, manager) = config_dir(1);

    let app = App::<()>::builder("reload-reentrancy-hook-test")
        .config_source(manager)
        .config::<SvcCfg>("svc")
        .component::<ReloadingHook>()
        .build()
        .await
        .expect("app builds");
    let reloader = app.reloader();

    write_value(&config_file, 2);

    let report = reload_within_timeout(&reloader)
        .await
        .expect("the outer reload keeps its normal outcome");
    let nested = recorded(&HOOK_NESTED);

    assert!(report.is_committed(), "the outer reload commits");
    assert_eq!(
        nested,
        [Nested::Reentrant(report.attempt())],
        "the hook's nested reload names the running attempt"
    );
    assert_eq!(
        app.container()
            .get::<ReloadingHook>()
            .expect("the hook owner resolves")
            .svc
            .get()
            .value,
        2,
        "the outer reload committed the new value"
    );

    write_value(&config_file, 3);

    let next = reload_within_timeout(&reloader)
        .await
        .expect("no lease leaked: a later reload runs");

    assert!(next.is_committed());
    assert_eq!(next.attempt().get(), report.attempt().get() + 1);
}

#[tokio::test]
async fn a_candidate_factory_reloading_its_own_runtime_gets_reentrant_reload() {
    let (_root, config_file, manager) = config_dir(1);

    let app = App::<()>::builder("reload-reentrancy-factory-test")
        .config_source(manager)
        .config::<SvcCfg>("svc")
        .component::<ReloadingFactory>()
        .build()
        .await
        .expect("app builds");
    let reloader = app.reloader();

    assert_eq!(
        recorded(&FACTORY_NESTED),
        [Nested::Unavailable],
        "the initial build runs before the reloader is attached"
    );

    write_value(&config_file, 2);

    let report = reload_within_timeout(&reloader)
        .await
        .expect("the outer reload keeps its normal outcome");

    assert!(report.is_committed(), "the outer reload commits");
    assert_eq!(
        recorded(&FACTORY_NESTED),
        [Nested::Unavailable, Nested::Reentrant(report.attempt())],
        "the reconstructing factory's nested reload names the running attempt"
    );
    assert_eq!(
        app.container()
            .get::<ReloadingFactory>()
            .expect("the reconstructed component resolves")
            .svc
            .get()
            .value,
        2
    );

    write_value(&config_file, 3);

    let next = reload_within_timeout(&reloader)
        .await
        .expect("no lease leaked: a later reload runs");

    assert!(next.is_committed());
}

#[tokio::test]
async fn an_a_to_b_to_a_reload_cycle_is_rejected_for_the_outer_runtime() {
    let (_root_a, config_a, manager_a) = config_dir(1);
    let (_root_b, config_b, manager_b) = config_dir(1);

    let app_a = App::<()>::builder("reload-reentrancy-cycle-a")
        .config_source(manager_a)
        .config::<CycleACfg>("cycle_a")
        .component::<CycleAHook>()
        .build()
        .await
        .expect("app A builds");
    let app_b = App::<()>::builder("reload-reentrancy-cycle-b")
        .config_source(manager_b)
        .config::<CycleBCfg>("cycle_b")
        .component::<CycleBHook>()
        .build()
        .await
        .expect("app B builds");
    let reloader_a = app_a.reloader();

    CYCLE_A_RELOADER
        .set(app_a.reloader())
        .unwrap_or_else(|_| panic!("runtime A's reloader is set once"));
    CYCLE_B_RELOADER
        .set(app_b.reloader())
        .unwrap_or_else(|_| panic!("runtime B's reloader is set once"));

    write_value(&config_a, 2);
    write_value(&config_b, 2);

    let report = reload_within_timeout(&reloader_a)
        .await
        .expect("the outer reload of A keeps its normal outcome");
    let a_nested = recorded(&CYCLE_A_NESTED);

    assert!(report.is_committed(), "the outer reload of A commits");
    assert_eq!(
        a_nested,
        [Nested::Committed],
        "A's hook reloads runtime B inline, which is not reentrant"
    );
    assert_eq!(
        recorded(&CYCLE_B_NESTED),
        [Nested::Reentrant(report.attempt())],
        "B's hook reloading A inside A's transaction is rejected with A's attempt"
    );
    assert_eq!(
        app_b
            .container()
            .get::<CycleBHook>()
            .expect("B's hook owner resolves")
            .cfg
            .get()
            .value,
        2,
        "the nested reload of B committed"
    );
    assert_eq!(
        app_a
            .container()
            .get::<CycleAHook>()
            .expect("A's hook owner resolves")
            .cfg
            .get()
            .value,
        2,
        "the outer reload of A committed"
    );
}
