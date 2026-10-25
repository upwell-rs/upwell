//! Entries in the prepared tooling document.

use upwell_tooling_schema::{RelationshipKind, ResourceKind};

use super::*;

#[test]
fn entries_project_with_their_scope_and_resolved_targets() {
    let prepared = builder()
        .with_component(Shared)
        .scope_entry::<Request, (Arc<Shared>,)>(key("test/tooled"))
        .prepare()
        .expect("application prepares");

    let document = prepared.tooling_document().expect("document projects");
    let entry = document
        .resources
        .iter()
        .find(|resource| resource.id == "entry:test/tooled")
        .expect("the entry is a resource");
    let edges = document
        .relationships
        .iter()
        .filter(|edge| edge.from == entry.id)
        .map(|edge| (edge.kind.clone(), edge.to.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(entry.kind, ResourceKind::Entry);
    assert_eq!(
        entry.labels.get("scope-id").map(String::as_str),
        Some("test/entry-request")
    );
    assert!(edges.contains(&(RelationshipKind::Enters, "scope:test/entry-request")));
    assert!(edges.contains(&(RelationshipKind::DependsOn, "component:shared")));
    document.validate().expect("the document validates");
}
