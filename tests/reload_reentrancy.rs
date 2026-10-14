//! Proof that a reload requested from inside a running transition of the same runtime —
//! from a config-reload hook or a candidate factory — fails fast with
//! `ReentrantReload` instead of deadlocking on the leases its own transaction holds,
//! while the outer reload keeps its normal outcome and releases every lease.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
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
            Ok(report) => Self::Other(format!("unexpected success: {report:?}")),
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
    fs::write(config_file, format!("[svc]\nvalue = {value}\n")).expect("write config");
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
