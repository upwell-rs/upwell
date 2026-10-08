//! Proof that a rejecting `#[hook(ConfigReload)]` aborts the whole reload (two-phase
//! all-or-nothing): nothing is committed and the live config keeps its old value.
#![cfg(feature = "daemon")]
#![allow(dead_code)]

use std::fs;

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::daemon::App;
use upwell::dirs::{Config, DirectoriesManager};
use upwell::{
    AppError, Cfg, CfgNext, ConfigManager, ConfigReload, ConfigReloadError, HookOutcome, component,
    config, methods,
};
use upwell_config::ResolverChain;

#[config(path = "svc")]
#[derive(Deserialize)]
struct SvcCfg {
    value: u32,
}

/// Always rejects a reload of `svc`.
#[component]
struct Rejector {
    #[config("svc")]
    svc: Cfg<SvcCfg>,
}

impl Rejector {
    fn committed(&self) -> u32 {
        self.svc.get().value
    }
}

#[methods]
impl Rejector {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("svc")] _next: CfgNext<SvcCfg>,
    ) -> upwell::daemon::Result<HookOutcome> {
        Err(upwell::daemon::Error::MissingComponent(
            "rejected by test hook",
        ))
    }
}

fn temp_config_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("upwell-hook-abort-")
        .tempdir()
        .expect("create temp config dir")
}

#[tokio::test]
async fn a_rejecting_hook_aborts_the_reload() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::builder("hook-abort-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let rejector = daemon
        .container()
        .get::<Rejector>()
        .expect("Rejector built");

    assert_eq!(rejector.committed(), 1, "starts at the file value");

    fs::write(&config_file, "[svc]\nvalue = 2\n").expect("rewrite config");

    let result = daemon.reloader().reload().await;

    assert!(
        matches!(
            &result,
            Err(AppError::ConfigReload(error)) if matches!(**error, ConfigReloadError::Hook { .. })
        ),
        "the rejecting hook surfaces as a hook error: {result:?}"
    );
    assert_eq!(
        rejector.committed(),
        1,
        "the value was NOT committed — the rejected reload rolled back"
    );

    // The reloader is still usable; the generation did not advance on the aborted reload.
    assert_eq!(
        daemon.runtime().config_generation(),
        0,
        "no successful reload yet"
    );
}
