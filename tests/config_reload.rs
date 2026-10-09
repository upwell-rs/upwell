//! End-to-end proof of manual config reloading: a file-backed app injects two
//! `Cfg<T>` bindings, one source value changes, and a runtime reload re-publishes **only**
//! the changed binding — the unchanged one keeps its exact `Arc` (no spurious swap),
//! and a snapshot taken before the reload stays pinned to the old value.

use std::any::TypeId;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::dirs::{Config, DirectoriesManager};
use upwell::{
    App, Cfg, ConfigManager, ConfigReloader, ConfigStore, RuntimeReloadReport, StagedConfig,
    StagedReload, component, config,
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

/// The config staging engine over `[svc]` and `[other]`, plus the active store whose
/// `Cfg` handles share the engine's live slots.
fn staging_engine(config_dir: &Path) -> (ConfigReloader, ConfigStore) {
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(config_dir, &[], ResolverChain::empty())
            .expect("load config")
            .with_config::<SvcCfg>("svc")
            .with_config::<OtherCfg>("other")
            .into_dynamic();
    let (store, slots) = ConfigStore::build(&manager).expect("bind config");

    (ConfigReloader::new(manager, slots), store)
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

    let report = daemon.reloader().reload().await.expect("reload succeeds");

    assert_eq!(
        report.config_generation, 1,
        "first successful reload is config generation 1"
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
        .reloader()
        .reload()
        .await
        .expect("second reload succeeds");

    assert!(
        !unchanged.published,
        "an unchanged source publishes nothing"
    );
    assert_eq!(
        unchanged.config_generation, 1,
        "an unchanged source does not advance the config generation"
    );
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

    let (reloader, store) = staging_engine(config_dir.path());
    let svc = store
        .resolve_path::<Cfg<SvcCfg>>("svc")
        .expect("active store resolves svc");
    let other = store
        .resolve_path::<Cfg<OtherCfg>>("other")
        .expect("active store resolves other");

    let unchanged = reloader.stage().expect("staging identical sources");

    assert!(
        unchanged.changed().is_empty(),
        "staging identical sources changes nothing"
    );
    assert_eq!(
        unchanged.changed_staged().count(),
        0,
        "an unchanged source exposes no changed staged entries"
    );

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged = reloader.stage().expect("staging after the source change");

    assert_eq!(staged.changed().len(), 1, "only one binding changed");
    assert_eq!(
        staged.changed()[0].path,
        "svc",
        "the changed binding is svc"
    );

    let changed_staged: Vec<_> = staged.changed_staged().collect();

    assert_eq!(
        changed_staged.len(),
        1,
        "only the changed binding is exposed as a staged entry"
    );
    assert_eq!(
        changed_staged[0].type_id(),
        TypeId::of::<SvcCfg>(),
        "the changed staged entry carries the bound type's exact identity"
    );
    assert_eq!(
        changed_staged[0].path(),
        "svc",
        "the changed staged entry carries the changed binding's path"
    );

    assert_eq!(
        svc.get().value,
        1,
        "the live value is untouched while the reload is only staged"
    );
    assert_eq!(
        other.get().value,
        100,
        "the unchanged binding keeps its value while staged"
    );

    staged.commit();

    assert_eq!(
        svc.get().value,
        2,
        "an explicit commit publishes the staged value"
    );
    assert_eq!(
        other.get().value,
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

    let (reloader, store) = staging_engine(config_dir.path());
    let svc = store
        .resolve_path::<Cfg<SvcCfg>>("svc")
        .expect("active store resolves svc");

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged = reloader.stage().expect("staging after the source change");
    let candidate = staged.candidate_store();

    assert!(
        Arc::ptr_eq(&candidate, &staged.candidate_store()),
        "repeated candidate_store calls return the same built store"
    );

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
        svc.get().value,
        1,
        "the active handle keeps the old value while staged"
    );
    assert_eq!(
        store
            .resolve_path::<Cfg<SvcCfg>>("svc")
            .expect("active store resolves svc")
            .get()
            .value,
        1,
        "the active store still resolves the old value while staged"
    );

    drop(staged);

    assert_eq!(
        svc.get().value,
        1,
        "dropping the staged reload publishes nothing"
    );
}

/// A no-op stage stays valid: nothing changed, every binding is staged, and the
/// candidate store still builds on demand — resolving every binding at its current
/// value while the active handles keep their state.
#[tokio::test]
async fn noop_stage_candidate_store_resolves_current_values() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let (reloader, store) = staging_engine(config_dir.path());
    let svc = store
        .resolve_path::<Cfg<SvcCfg>>("svc")
        .expect("active store resolves svc");

    let staged = reloader.stage().expect("staging identical sources");

    assert!(
        staged.changed().is_empty(),
        "an unchanged source changes nothing"
    );
    assert_eq!(staged.staged().len(), 2, "every binding is staged");

    let candidate = staged.candidate_store();

    assert_eq!(
        candidate
            .resolve_path::<Cfg<SvcCfg>>("svc")
            .expect("candidate store resolves svc")
            .get()
            .value,
        1,
        "the candidate store holds the unchanged binding's current value"
    );
    assert_eq!(
        candidate
            .resolve_path::<Cfg<OtherCfg>>("other")
            .expect("candidate store resolves other")
            .get()
            .value,
        100,
        "the candidate store holds the unchanged binding's current value"
    );

    assert_eq!(
        svc.get().value,
        1,
        "the active handle is untouched by staging and candidate resolution"
    );
}

/// The facade root re-exports the staged reload API: `ConfigReloader::stage`'s return
/// type is nameable as `StagedReload` and the staged/proposal input type as
/// `StagedConfig`, both through `upwell::` — so facade-only users can stage a reload
/// and inspect staged entries without depending on `upwell-config` directly.
#[tokio::test]
async fn facade_root_exposes_staged_reload_and_staged_config() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let (reloader, _) = staging_engine(config_dir.path());

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged: StagedReload = reloader.stage().expect("staging after the source change");

    let staged_entries: &[StagedConfig] = staged.staged();

    assert_eq!(staged_entries.len(), 2, "every binding is staged");
    assert_eq!(
        staged_entries
            .iter()
            .find(|entry| entry.path() == "svc")
            .map(|entry| entry.type_id()),
        Some(TypeId::of::<SvcCfg>()),
        "the staged entry carries the bound type's exact identity through the facade"
    );
}

/// The facade root re-exports the transactional reload report: `AppRuntime::reload_config`'s
/// return type is nameable as `RuntimeReloadReport` through `upwell::` — so facade-only
/// users can run a graph-transitioning reload and inspect its report without depending on
/// `upwell-app` directly.
#[tokio::test]
async fn facade_root_exposes_runtime_reload_report() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-facade-reload-report-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let report: RuntimeReloadReport = daemon
        .runtime()
        .reload_config()
        .await
        .expect("reload succeeds");

    assert!(
        report.published,
        "a changed source publishes a new runtime generation"
    );
    assert_eq!(report.changed.len(), 1, "only one binding changed");
    assert_eq!(report.changed[0].path, "svc", "the changed binding is svc");
    assert!(
        report.hooks.is_empty(),
        "no config-reload hooks are registered in this app"
    );
    assert_eq!(
        report.config_generation, 1,
        "the first commit advances the config generation to 1"
    );
}

/// The facade root re-exports the reload failure payload and runtime-state types:
/// `RestartRequired`/`RestartReason` — the documented failure mode of
/// `AppRuntime::reload_config` — plus `AppConditionState` and `PreparedScopeTopology`
/// (the type behind `AppRuntime::scope_topology`), so facade-only users can name and
/// match them without depending on `upwell-app` directly.
#[tokio::test]
async fn facade_root_names_reload_failure_and_runtime_state_types() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-facade-reload-state-types-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let topology: Arc<upwell::PreparedScopeTopology> = daemon.runtime().scope_topology();

    assert!(
        topology.boundaries().is_empty(),
        "the prepared scope topology is consumable through the facade"
    );

    fn restart_payload(error: &upwell::AppError) -> Option<(&'static str, upwell::RestartReason)> {
        match error {
            upwell::AppError::RestartRequired(required) => {
                Some((required.component, required.reason))
            }
            _ => None,
        }
    }

    let restart = upwell::AppError::RestartRequired(upwell::RestartRequired {
        component: "svc",
        required: None,
        reason: upwell::RestartReason::NonSingleton,
    });

    assert_eq!(
        restart_payload(&restart),
        Some(("svc", upwell::RestartReason::NonSingleton)),
        "the facade exposes the complete restart-required payload"
    );

    assert_eq!(
        restart_payload(&upwell::AppError::CandidatePreparationPanicked),
        None,
        "a non-restart error carries no restart payload through the facade"
    );

    // `AppConditionState` has no public constructor — it is carried by committed
    // generations — so naming coverage spells it in a signature instead.
    let _condition_catalog: fn(&upwell::AppConditionState) -> &Arc<upwell::AppRegistry> =
        upwell::AppConditionState::catalog;
}

/// Requests reloads through the injected framework handle.
#[component]
struct ReloadAdmin {
    reloader: upwell::RuntimeReloader,
}

/// A component injects the facade's [`RuntimeReloader`](upwell::RuntimeReloader) and
/// drives the transactional reload with it.
#[tokio::test]
async fn components_inject_the_runtime_reloader() {
    let root = temp_config_dir();
    let dirs = DirectoriesManager::from_path(root.path().to_path_buf());
    let config_dir = dirs.dir::<Config>();
    let config_file = config_dir.path().join("application.toml");

    fs::create_dir_all(config_dir.path()).expect("create config subdir");
    fs::write(&config_file, "[svc]\nvalue = 1\n\n[other]\nvalue = 100\n").expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let daemon = App::<()>::builder("config-injected-reloader-test")
        .config_source(manager)
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");
    let admin = daemon
        .container()
        .get::<ReloadAdmin>()
        .expect("ReloadAdmin constructed");
    let before = daemon.runtime().generation();

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let report = admin
        .reloader
        .reload()
        .await
        .expect("the injected reloader reloads");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(
        daemon.runtime().generation().get(),
        before.get() + 1,
        "the injected handle transitions the app's runtime"
    );
}
