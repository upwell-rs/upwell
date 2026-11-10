//! Builder and plugin registration through full application preparation.

use upwell_core::{DescriptorSource, ItemSource};

use super::*;
use crate::{Plugin, PluginContributionKind, PluginContributions, PluginId};

/// A plugin contributing one session entry.
#[derive(Default)]
struct EntryPlugin;

impl Plugin for EntryPlugin {
    const ID: PluginId = upwell_core::namespaced_id!(PluginId, "test/entry-plugin");

    fn contribute(self, contributions: &mut PluginContributions) {
        crate::contribute! {
            to contributions,
            entries: [
                "test/plugin-entry" => EntryDescriptor::new::<Session, ()>(key("test/plugin")),
            ],
        }
    }
}

#[test]
fn protocol_plugin_and_builder_entries_share_one_table() {
    let prepared = builder()
        .register_plugin::<EntryPlugin>()
        .scope_entry::<Singleton, ()>(key("test/builder"))
        .prepare()
        .expect("application prepares");

    let keys = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();
    let kinds = prepared
        .plugin_plan()
        .emitted_contributions()
        .iter()
        .map(|contribution| contribution.kind())
        .collect::<Vec<_>>();

    assert_eq!(keys, ["test/builder", "test/plugin", "test/protocol"]);
    assert_eq!(kinds, [PluginContributionKind::Entry]);
}

#[test]
fn preparation_fails_for_an_entry_with_an_unresolvable_target() {
    let result = builder()
        .scope_entry::<Session, (Arc<Unregistered>,)>(key("test/unresolvable"))
        .prepare();

    assert!(matches!(
        result,
        Err(Error::Entry(EntryError::MissingDependency { key, .. }))
            if key.as_str() == "test/unresolvable"
    ));
}

#[test]
fn one_declaration_registered_twice_is_kept_once() {
    let declared =
        EntryDescriptor::new::<Session, ()>(key("test/declared")).declared_at(source("declared"));

    let prepared = builder()
        .entry_descriptor(declared)
        .entry_descriptor(declared)
        .prepare()
        .expect("application prepares");

    let keys = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();

    assert_eq!(keys, ["test/declared", "test/protocol"]);
}

#[test]
fn distinct_or_undeclared_entries_sharing_a_key_are_rejected() {
    let first =
        EntryDescriptor::new::<Session, ()>(key("test/shared")).declared_at(source("first"));
    let second =
        EntryDescriptor::new::<Session, ()>(key("test/shared")).declared_at(source("second"));
    let undeclared = EntryDescriptor::new::<Session, ()>(key("test/shared"));

    let declared = builder()
        .entry_descriptor(first)
        .entry_descriptor(second)
        .prepare();
    let undeclared = builder()
        .entry_descriptor(undeclared)
        .entry_descriptor(undeclared)
        .prepare();

    for result in [declared, undeclared] {
        assert!(matches!(
            result,
            Err(Error::Entry(EntryError::DuplicateKey { key }))
                if key.as_str() == "test/shared"
        ));
    }
}

fn source(item: &'static str) -> ItemSource {
    ItemSource::new(
        DescriptorSource::new(file!(), line!(), 1),
        module_path!(),
        item,
    )
}
