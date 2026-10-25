//! Builder and plugin registration through full application preparation.

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
