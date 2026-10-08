//! Phase 4 triggers: `ConfigManager` carries the opt-in reload triggers (config lives on the
//! manager, never a protocol), an app builder can be configured with that manager, and — under the
//! `watch` feature — a file change drives the app runtime's transactional reload.
#![allow(dead_code)]

use std::fs;
use std::time::Duration;

use tempfile::TempDir;
use upwell::ConfigManager;
use upwell::config::Toml;
#[cfg(any(feature = "daemon", feature = "watch"))]
use upwell::dirs::{Config, DirectoriesManager};
use upwell_config::ResolverChain;
use upwell_test_utils::AbortOnDropTask;

#[cfg(feature = "watch")]
use serde::Deserialize;
#[cfg(feature = "watch")]
use upwell::{App, config};

/// The bound `[demo]` section, so a file edit is a real binding change that the
/// transactional reload publishes rather than an unchanged-source no-op.
#[cfg(feature = "watch")]
#[config]
#[derive(Deserialize)]
struct DemoConfig {
    value: u32,
}

fn temp_dir(tag: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("upwell-triggers-{tag}-"))
        .tempdir()
        .expect("create temp dir")
}

#[test]
fn config_manager_carries_its_triggers() {
    let manager = ConfigManager::<Toml>::empty()
        .with_resolvers(ResolverChain::empty())
        .reload_on_sighup()
        .watch_config()
        .config_reload_debounce(Duration::from_millis(123));

    let triggers = manager.triggers();

    assert!(triggers.sighup, "sighup requested");
    assert!(triggers.watch, "watch requested");
    assert_eq!(triggers.debounce, Duration::from_millis(123));
}

#[tokio::test]
#[cfg(feature = "daemon")]
async fn app_builder_builds_with_a_configured_manager() -> upwell::daemon::Result<()> {
    let root = temp_dir("macro");
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());

    fs::create_dir_all(dirs.dir::<Config>().path()).expect("create config dir");
    fs::write(dirs.dir::<Config>().join("application.toml"), "").expect("write config");

    let config = ConfigManager::<upwell::config::Dynamic>::load_from_with_resolvers(
        &dirs,
        &[],
        ResolverChain::empty(),
    )?
    .reload_on_sighup()
    .config_reload_debounce(Duration::from_millis(50));

    let built = upwell::App::<upwell::daemon::Rpc>::builder("trigger-builder-test")
        .auto_discover()
        .directories(dirs)
        .config_source(config)
        .build()
        .await?;

    // The reloader is always present; a manual reload still works.
    let report = built
        .reloader()
        .reload()
        .await
        .expect("manual reload works");

    assert!(report.changed.is_empty(), "nothing changed on first reload");

    Ok(())
}

#[cfg(feature = "watch")]
#[tokio::test]
async fn watching_a_source_file_triggers_a_reload() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("watch");
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config dir");
    fs::write(&config_file, "[demo]\nvalue = 1\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_from_with_resolvers(&dirs, &[], ResolverChain::empty())
            .expect("load config")
            .watch_config()
            .config_reload_debounce(Duration::from_millis(50));

    let app = App::<()>::builder("watch-test")
        .config_source(manager)
        .config::<DemoConfig>("demo")
        .build()
        .await
        .expect("protocol-neutral app builds");

    let runtime = app.runtime().clone();
    let shutdown = app.shutdown_handle();
    let before = runtime.config_generation();
    let runtime_before = runtime.generation();

    let mut task = AbortOnDropTask::spawn("watch daemon", app.run());
    let mut daemon_exit = None;
    let mut reloaded = false;

    for value in 2..=31 {
        fs::write(&config_file, format!("[demo]\nvalue = {value}\n")).expect("rewrite config");

        tokio::select! {
            result = task.join() => {
                daemon_exit = Some(result);
                break;
            }
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }

        if runtime.config_generation() > before {
            reloaded = true;
            break;
        }
    }

    let exited_early = daemon_exit.is_some() || task.is_finished();

    shutdown.shutdown();
    let daemon_result = match daemon_exit {
        Some(result) => result,
        None => task.join_with_timeout(Duration::from_secs(2)).await,
    };

    daemon_result?;

    assert!(!exited_early, "daemon exited before a reload was observed");
    assert!(reloaded, "a config file change triggered a reload");
    assert!(
        runtime.generation() > runtime_before,
        "the triggered reload published a runtime generation, not only config"
    );

    Ok(())
}
