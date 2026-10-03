//! End-to-end proof of manual config reloading: a file-backed app injects two
//! `Cfg<T>` bindings, one source value changes, and a reload re-publishes **only**
//! the changed binding — the unchanged one keeps its exact `Arc` (no spurious swap),
//! and a snapshot taken before the reload stays pinned to the old value.

use std::fs;
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;
use upwell::ContainerConfigExt;
use upwell::config::Toml;
use upwell::dirs::{Config, DirectoriesManager};
use upwell::{App, Cfg, ConfigManager, component, config};
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

/// Holds two config bindings so a reload can be observed to touch only one.
#[component]
struct Consumer {
    #[config("svc")]
    svc: Cfg<SvcCfg>,
    #[config("other")]
    other: Cfg<OtherCfg>,
}

impl Consumer {
    fn svc(&self) -> &Cfg<SvcCfg> {
        &self.svc
    }

    fn other(&self) -> &Cfg<OtherCfg> {
        &self.other
    }
}

fn temp_config_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("upwell-config-reload-")
        .tempdir()
        .expect("create temp config dir")
}

#[tokio::test]
async fn reload_swaps_only_the_changed_binding() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-reload-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let consumer = daemon
        .container()
        .get::<Consumer>()
        .expect("Consumer constructed");

    let svc_before = consumer.svc().snapshot();
    let other_before = consumer.other().snapshot();

    assert_eq!(svc_before.value, 1, "svc starts at the file value");
    assert_eq!(other_before.value, 100, "other starts at the file value");

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let report = daemon
        .config_reloader()
        .reload()
        .await
        .expect("reload succeeds");

    assert_eq!(
        report.generation, 1,
        "first successful reload is generation 1"
    );
    assert_eq!(report.changed.len(), 1, "only one binding changed");
    assert_eq!(report.changed[0].path, "svc", "the changed binding is svc");

    assert_eq!(
        consumer.svc().get().value,
        2,
        "the changed binding observes the new value"
    );
    assert_eq!(
        consumer.other().get().value,
        100,
        "the unchanged binding keeps its value"
    );

    assert!(
        Arc::ptr_eq(&other_before, &consumer.other().snapshot()),
        "the unchanged binding was not re-published (same Arc, no spurious swap)"
    );
    assert!(
        !Arc::ptr_eq(&svc_before, &consumer.svc().snapshot()),
        "the changed binding was actually swapped"
    );
    assert_eq!(
        svc_before.value, 1,
        "a snapshot taken before the reload stays pinned to the old value"
    );

    let unchanged = daemon
        .config_reloader()
        .reload()
        .await
        .expect("second reload succeeds");

    assert_eq!(unchanged.generation, 2, "generation advances every reload");
    assert!(
        unchanged.changed.is_empty(),
        "re-reading identical sources changes nothing"
    );
}

/// A staged reload is inert until explicitly committed: staging reports the changed
/// bindings and staged values without touching any live slot, and only `commit()`
/// publishes them. This is the primitive the transactional config-and-graph commit
/// builds on.
#[tokio::test]
async fn staged_reload_commits_only_when_explicitly_committed() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-staged-reload-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let consumer = daemon
        .container()
        .get::<Consumer>()
        .expect("Consumer constructed");
    let reloader = daemon.config_reloader();

    let unchanged = reloader.stage().expect("staging identical sources");

    assert!(
        unchanged.changed().is_empty(),
        "staging identical sources changes nothing"
    );

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged = reloader.stage().expect("staging after the source change");

    assert_eq!(staged.changed().len(), 1, "only one binding changed");
    assert_eq!(
        staged.changed()[0].path,
        "svc",
        "the changed binding is svc"
    );

    assert_eq!(
        consumer.svc().get().value,
        1,
        "the live value is untouched while the reload is only staged"
    );
    assert_eq!(
        consumer.other().get().value,
        100,
        "the unchanged binding keeps its value while staged"
    );

    staged.commit();

    assert_eq!(
        consumer.svc().get().value,
        2,
        "an explicit commit publishes the staged value"
    );
    assert_eq!(
        consumer.other().get().value,
        100,
        "the unchanged binding keeps its value after the commit"
    );
}

/// A staged reload exposes a generation-local candidate store: every binding — changed
/// and unchanged — is re-seeded into fresh `Cfg` cells holding the values the reload
/// would publish, while the active handles and the active store keep the old values
/// until an explicit commit. Dropping the staged reload publishes nothing.
#[tokio::test]
async fn staged_candidate_store_resolves_proposed_values_without_touching_active_handles() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-candidate-store-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let consumer = daemon
        .container()
        .get::<Consumer>()
        .expect("Consumer constructed");
    let reloader = daemon.config_reloader();

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged = reloader.stage().expect("staging after the source change");
    let candidate = staged.candidate_store();

    assert_eq!(
        candidate
            .resolve_path::<Cfg<SvcCfg>>("svc")
            .expect("candidate store resolves the changed binding")
            .get()
            .value,
        2,
        "the candidate store resolves the proposed value for the changed binding"
    );
    assert_eq!(
        candidate
            .resolve_path::<Cfg<OtherCfg>>("other")
            .expect("candidate store resolves the unchanged binding")
            .get()
            .value,
        100,
        "the candidate store contains the unchanged binding at its current value"
    );

    assert_eq!(
        consumer.svc().get().value,
        1,
        "the active handle keeps the old value while staged"
    );
    assert_eq!(
        daemon
            .container()
            .config::<SvcCfg>("svc")
            .expect("active store resolves svc")
            .get()
            .value,
        1,
        "the active store still resolves the old value while staged"
    );

    drop(staged);

    assert_eq!(
        consumer.svc().get().value,
        1,
        "dropping the staged reload publishes nothing"
    );
}
