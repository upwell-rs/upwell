//! Entries in the prepared tooling document.

use std::collections::BTreeMap;

use upwell_di::ScopedConstruction;
use upwell_tooling_schema::{RelationshipKind, ResourceKind, ToolingDocument};

use super::*;
use crate::test_support::{dependency, unbuilt_factories};

/// A request-scoped component no entry reaches.
struct Idle;

test_component!(Idle, "idle");

/// [`RequestData`], built by a factory on every opening of the request scope.
static EAGER_REQUEST_DATA: ComponentDescriptor = ComponentDescriptor {
    factories: unbuilt_factories!("eager", Vec::new),
    construction: ScopedConstruction::Eager,
    ..ComponentDescriptor::manual(
        RequestData::ID,
        RequestData::NAME,
        TypeDescriptor::of::<RequestData>(RequestData::NAME),
        Request::KEY,
    )
};

/// [`Idle`], built by a factory on demand.
static IDLE: ComponentDescriptor = ComponentDescriptor {
    factories: unbuilt_factories!("idle", Vec::new),
    ..ComponentDescriptor::manual(
        Idle::ID,
        Idle::NAME,
        TypeDescriptor::of::<Idle>(Idle::NAME),
        Request::KEY,
    )
};

/// The labels of resource `id` in `document`.
fn labels<'a>(document: &'a ToolingDocument, id: &str) -> &'a BTreeMap<String, String> {
    &document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .unwrap_or_else(|| panic!("{id} is a resource"))
        .labels
}

fn label<'a>(labels: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    labels.get(name).map(String::as_str)
}

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

/// [`SessionStore`], built by a factory on demand.
static SESSION_STORE: ComponentDescriptor = ComponentDescriptor {
    factories: unbuilt_factories!("session-store", Vec::new),
    ..ComponentDescriptor::manual(
        SessionStore::ID,
        SessionStore::NAME,
        TypeDescriptor::of::<SessionStore>(SessionStore::NAME),
        Session::KEY,
    )
};

/// [`RequestData`], built by a factory reading [`SessionStore`] from the scope around it.
static SESSION_READER: ComponentDescriptor = ComponentDescriptor {
    factories: unbuilt_factories!("session-reader", || {
        vec![dependency(
            "SessionStore",
            TypeDescriptor::of::<SessionStore>(SessionStore::NAME),
            upwell_core::Cardinality::One,
            upwell_core::ResolutionMode::Eager,
        )]
    }),
    ..ComponentDescriptor::manual(
        RequestData::ID,
        RequestData::NAME,
        TypeDescriptor::of::<RequestData>(RequestData::NAME),
        Request::KEY,
    )
};

#[test]
fn a_component_read_from_a_child_scope_entry_is_reached() {
    let prepared = builder()
        .component_descriptor(&SESSION_STORE)
        .component_descriptor(&SESSION_READER)
        .scope_entry::<Session, ()>(key("test/session"))
        .scope_entry::<Request, (Arc<RequestData>,)>(key("test/reader"))
        .prepare()
        .expect("application prepares");

    let document = prepared.tooling_document().expect("document projects");
    let session = labels(&document, "scope:test/entry-session");

    assert_eq!(
        label(session, "unreached-by-entries"),
        Some("none"),
        "the session entry plans nothing, but a request entry reads the store"
    );
}

#[test]
fn scopes_report_their_eager_set_and_what_no_entry_reaches() {
    let prepared = builder()
        .with_component(Shared)
        .component_descriptor(&EAGER_REQUEST_DATA)
        .component_descriptor(&IDLE)
        .scope_entry::<Request, (Arc<Shared>,)>(key("test/tooled"))
        .prepare()
        .expect("application prepares");

    let document = prepared.tooling_document().expect("document projects");
    let request = labels(&document, "scope:test/entry-request");
    let session = labels(&document, "scope:test/entry-session");
    let entry = labels(&document, "entry:test/tooled");
    let eager = labels(&document, "component:request_data");
    let idle = labels(&document, "component:idle");

    assert_eq!(label(request, "eager"), Some("request_data"));
    assert_eq!(label(request, "unreached-by-entries"), Some("idle"));
    assert_eq!(label(session, "eager"), Some("none"));
    assert_eq!(
        label(session, "unreached-by-entries"),
        None,
        "no entry opens the session scope"
    );
    assert_eq!(label(entry, "builds"), Some("request_data"));
    assert_eq!(label(entry, "plan-size"), Some("1"));
    assert_eq!(label(entry, "reads-seeds"), Some("none"));
    assert_eq!(label(request, "seeds"), Some("none"));
    assert_eq!(label(eager, "construction-policy"), Some("eager"));
    assert_eq!(label(idle, "construction-policy"), Some("on-demand"));
    document.validate().expect("the document validates");
}
