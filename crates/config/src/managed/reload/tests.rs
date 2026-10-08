//! Unit tests for the staged reload's candidate store: staging never seeds the
//! generation-local candidate store, and the first
//! [`StagedReload::candidate_store`](super::StagedReload::candidate_store) call builds
//! it exactly once from the staged entries.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;

use crate::ResolverChain;
use crate::managed::{Cfg, ConfigManager, ConfigProperties, ConfigReloader, ConfigStore, Toml};

#[derive(Debug, Deserialize)]
struct SvcCfg {
    value: u32,
}

impl ConfigProperties for SvcCfg {
    const NAME: &'static str = "SvcCfg";
}

#[derive(Debug, Deserialize)]
struct OtherCfg {
    value: u32,
}

impl ConfigProperties for OtherCfg {
    const NAME: &'static str = "OtherCfg";
}

/// A file-backed reloader over two bindings (`svc`, `other`), with the config file
/// path returned so a test can rewrite the source between stages.
fn two_binding_reloader(initial: &str) -> (TempDir, PathBuf, ConfigReloader) {
    let root = tempfile::Builder::new()
        .prefix("upwell-config-candidate-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = root.path().join("config");
    let config_file = config_dir.join("application.toml");

    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(&config_file, initial).expect("write config");

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config")
            .with_config::<SvcCfg>("svc")
            .with_config::<OtherCfg>("other")
            .into_dynamic();

    let (_store, slots) = ConfigStore::build(&manager).expect("bind config");

    (root, config_file, ConfigReloader::new(manager, slots))
}

#[test]
fn staged_reload_builds_the_candidate_store_lazily_and_exactly_once() {
    let (_dir, config_file, reloader) =
        two_binding_reloader("[svc]\nvalue = 1\n\n[other]\nvalue = 100\n");

    fs::write(&config_file, "[svc]\nvalue = 2\n\n[other]\nvalue = 100\n").expect("rewrite config");

    let staged = reloader.stage().expect("staging after the source change");

    assert_eq!(staged.changed().len(), 1, "only svc changed");
    assert!(
        !staged.candidate_is_built(),
        "staging must not build the candidate store"
    );

    let first = staged.candidate_store();

    assert!(
        staged.candidate_is_built(),
        "the first candidate_store call builds the store"
    );

    let second = staged.candidate_store();

    assert!(
        Arc::ptr_eq(&first, &second),
        "repeated candidate_store calls return the same built store"
    );
    assert_eq!(
        first
            .resolve_path::<Cfg<SvcCfg>>("svc")
            .expect("candidate store resolves svc")
            .get()
            .value,
        2,
        "the candidate store holds the changed binding's proposed value"
    );
    assert_eq!(
        first
            .resolve_path::<Cfg<OtherCfg>>("other")
            .expect("candidate store resolves other")
            .get()
            .value,
        100,
        "the candidate store holds the unchanged binding's value"
    );
}

#[test]
fn noop_stage_leaves_the_candidate_store_unbuilt_until_requested() {
    let (_dir, _config_file, reloader) =
        two_binding_reloader("[svc]\nvalue = 1\n\n[other]\nvalue = 100\n");

    let staged = reloader.stage().expect("staging identical sources");

    assert!(
        staged.changed().is_empty(),
        "an unchanged source changes nothing"
    );
    assert_eq!(staged.staged().len(), 2, "every binding is staged");
    assert!(
        !staged.candidate_is_built(),
        "a no-op stage must not build the candidate store"
    );

    let candidate = staged.candidate_store();

    assert!(
        staged.candidate_is_built(),
        "candidate_store builds on demand"
    );
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
}
