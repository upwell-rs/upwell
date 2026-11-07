//! `#[entry]` functions: discovery, explicit registration, keys, scopes and targets.

#![cfg(feature = "daemon")]

use std::sync::Arc;

use upwell::config::Toml;
use upwell::daemon::{App, Rpc};
use upwell::scope::{Request, Singleton};
use upwell::{
    AppBuilder, Cfg, ConfigManager, Descriptor, EntryDescriptor, Scope, component, config, entry,
};

/// A singleton every entry injects.
#[component]
struct Ledger;

/// Config one entry injects.
#[config(path = "sync")]
#[derive(serde::Deserialize)]
struct SyncConfig {
    #[allow(dead_code)]
    batch: u32,
}

/// Reconciles the ledger in the root scope under its derived key.
#[entry(scope = Singleton)]
async fn reconcile(_ledger: Arc<Ledger>, _config: Cfg<SyncConfig>) {}

/// Audits one request under an explicit key.
#[entry(scope = Request, key = "app/ledger.audit")]
fn audit_request(_ledger: Arc<Ledger>) {}

#[test]
fn discovered_entries_carry_their_keys_scopes_and_targets() {
    let prepared = builder("entry-macro-discovery")
        .prepare()
        .expect("app prepares");

    let entries = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| (entry.key.as_str(), entry.scope, target_names(entry)))
        .collect::<Vec<_>>();

    assert_eq!(
        entries,
        [
            ("app/ledger.audit", Request::KEY, vec!["Ledger"]),
            (
                "app/reconcile",
                Singleton::KEY,
                vec!["Ledger", "SyncConfig"]
            ),
        ]
    );
}

#[test]
fn explicit_registration_of_a_discovered_entry_keeps_it_once() {
    let prepared = builder("entry-macro-explicit")
        .entry::<Reconcile>()
        .prepare()
        .expect("app prepares");

    let keys = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();

    assert_eq!(keys, ["app/ledger.audit", "app/reconcile"]);
}

#[test]
fn the_marker_records_the_function_declaration() {
    let descriptor = <AuditRequest as Descriptor<EntryDescriptor>>::DESCRIPTOR;
    let source = descriptor
        .source
        .expect("the macro records the declaration");

    assert!(descriptor.owner.is_none());
    assert_eq!(source.item, "audit_request");
    assert_eq!(source.module_path, "entry_macro");
}

fn builder(name: &str) -> AppBuilder<Rpc> {
    let config = ConfigManager::<Toml>::from_str("[sync]\nbatch = 10\n").expect("config parses");

    App::builder(name).auto_discover().config_source(config)
}

fn target_names(entry: &EntryDescriptor) -> Vec<&'static str> {
    let mut names = (entry.targets)()
        .iter()
        .map(|target| target.name)
        .collect::<Vec<_>>();

    names.sort_unstable();

    names
}
