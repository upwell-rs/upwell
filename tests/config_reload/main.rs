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
    App, Cfg, ConfigManager, ConfigReloader, ConfigStore, RuntimeTransitionReport, StagedConfig,
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

mod facade;
mod staging;
