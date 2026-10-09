//! End-to-end proof of `#[hook(ConfigReload)]`: a reload fires a component's hook with the
//! **proposed** value, but only for components whose declared config actually changed, and
//! the per-hook outcomes surface in the report.
#![allow(dead_code)]

use std::fs;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::dirs::{Config, DirectoriesManager};
use upwell::{
    App, AppError, Cfg, CfgNext, ConfigManager, ConfigReload, HookOutcome, component, config,
    methods,
};
use upwell_config::ResolverChain;

#[config(path = "svc")]
#[derive(Deserialize)]
struct SvcCfg {
    value: u32,
}

#[config(path = "other")]
#[derive(Deserialize)]
struct OtherCfg {
    value: u32,
}

/// Reacts to changes of the `svc` config, recording the proposed value it was handed.
#[component]
struct Watcher {
    #[config("svc")]
    svc: Cfg<SvcCfg>,
    #[default]
    fired: AtomicUsize,
    #[default]
    last_seen: AtomicU32,
}

impl Watcher {
    fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }

    fn last_seen(&self) -> u32 {
        self.last_seen.load(Ordering::SeqCst)
    }

    fn committed(&self) -> u32 {
        self.svc.get().value
    }
}

#[methods]
impl Watcher {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("svc")] next: CfgNext<SvcCfg>,
    ) -> Result<HookOutcome, AppError> {
        self.last_seen.store(next.value, Ordering::SeqCst);
        self.fired.fetch_add(1, Ordering::SeqCst);

        Ok(HookOutcome::Reloaded)
    }
}

/// Reacts only to the `other` config — must stay silent when only `svc` changes.
#[component]
struct OtherWatcher {
    #[default]
    fired: AtomicUsize,
}

impl OtherWatcher {
    fn fired(&self) -> usize {
        self.fired.load(Ordering::SeqCst)
    }
}

#[methods]
impl OtherWatcher {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("other")] _next: CfgNext<OtherCfg>,
    ) -> Result<HookOutcome, AppError> {
        self.fired.fetch_add(1, Ordering::SeqCst);

        Ok(HookOutcome::Reloaded)
    }
}

/// Also reacts to `svc`, but reports that a restart is required.
#[component]
struct RestartWatcher {
    #[default]
    marker: u8,
}

#[methods]
impl RestartWatcher {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("svc")] _next: CfgNext<SvcCfg>,
    ) -> Result<HookOutcome, AppError> {
        let _ = self.marker;

        Ok(HookOutcome::RestartRequired("needs restart"))
    }
}

fn temp_config_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("upwell-hooks-")
        .tempdir()
        .expect("create temp config dir")
}

#[tokio::test]
async fn config_reload_hooks_fire_only_for_changed_configs() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("hooks-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let watcher = daemon.container().get::<Watcher>().expect("Watcher built");

    assert_eq!(watcher.fired(), 0, "no reload yet");

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let report = daemon.reloader().reload().await.expect("reload succeeds");

    // Hook owners reconstruct in the published generation, so its instances carry the
    // hook-observed state.
    let watcher = daemon
        .container()
        .get::<Watcher>()
        .expect("the published Watcher resolves");
    let other = daemon
        .container()
        .get::<OtherWatcher>()
        .expect("the published OtherWatcher resolves");

    assert_eq!(watcher.fired(), 1, "the svc watcher fired exactly once");
    assert_eq!(
        watcher.last_seen(),
        2,
        "the hook received the proposed (new) value"
    );
    assert_eq!(
        watcher.committed(),
        2,
        "the value was committed after the hook accepted"
    );
    assert_eq!(
        other.fired(),
        0,
        "the other watcher did not fire — its config did not change"
    );

    // Both svc-targeting hooks (Watcher + RestartWatcher) ran; the other-targeting one did not.
    let outcomes: Vec<HookOutcome> = report.hooks.iter().map(|h| h.outcome).collect();
    assert_eq!(report.hooks.len(), 2, "two svc hooks ran: {outcomes:?}");
    assert!(
        outcomes.contains(&HookOutcome::Reloaded),
        "Watcher reported Reloaded"
    );
    assert!(
        outcomes.contains(&HookOutcome::RestartRequired("needs restart")),
        "RestartWatcher's RestartRequired surfaced in the report"
    );

    // A no-op reload fires no hooks.
    let again = daemon
        .reloader()
        .reload()
        .await
        .expect("second reload succeeds");

    assert!(
        again.hooks.is_empty(),
        "no config changed, so no hooks fired"
    );
    assert_eq!(watcher.fired(), 1, "watcher did not fire again");
}
