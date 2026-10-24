//! The facade root's reload, transition-report and runtime-state API, and components that
//! inject the runtime reloader.

use super::*;

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
/// return type is nameable as `RuntimeTransitionReport` through `upwell::` — so facade-only
/// users can run a graph-transitioning reload and inspect its report without depending on
/// `upwell-app` directly.
#[tokio::test]
async fn facade_root_exposes_runtime_transition_report() {
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

    let report: RuntimeTransitionReport = daemon
        .runtime()
        .reload_config()
        .await
        .expect("reload succeeds");

    assert!(
        report.is_committed(),
        "a changed source publishes a new runtime generation"
    );
    assert_eq!(report.changed().len(), 1, "only one binding changed");
    assert_eq!(
        report.changed()[0].path,
        "svc",
        "the changed binding is svc"
    );
    assert!(
        report.hooks().is_empty(),
        "no config-reload hooks are registered in this app"
    );
    assert_eq!(
        report.config_generation(),
        1,
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

    assert!(
        report.is_committed(),
        "a changed source publishes a generation"
    );
    assert_eq!(
        daemon.runtime().generation().get(),
        before.get() + 1,
        "the injected handle transitions the app's runtime"
    );
}
