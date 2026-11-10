//! The dense entry table: key order, identity lookups, and scope admission.

use super::*;

#[test]
fn entries_are_indexed_densely_in_key_order() {
    let entries = [
        EntryDescriptor::new::<Request, ()>(key("test/c-request")),
        EntryDescriptor::new::<Singleton, ()>(key("test/a-root")),
        EntryDescriptor::new::<Session, ()>(key("test/b-session")),
    ];

    let table = EntryTable::new(&entries, &topology()).expect("entries prepare");
    let ids = table
        .iter()
        .map(|(id, entry)| (id.index(), entry.key.as_str()))
        .collect::<Vec<_>>();
    let session = table
        .id(&key("test/b-session"))
        .expect("session entry is indexed");

    assert_eq!(
        ids,
        [
            (0, "test/a-root"),
            (1, "test/b-session"),
            (2, "test/c-request")
        ]
    );
    assert_eq!(session.index(), 1);
    assert_eq!(
        table.get(session).map(|entry| entry.scope),
        Some(Session::KEY)
    );
    assert_eq!(table.id(&key("test/absent")), None);
    assert_eq!(table.len(), 3);
}

#[test]
fn duplicate_keys_are_rejected() {
    let entries = [
        EntryDescriptor::new::<Session, ()>(key("test/twice")),
        EntryDescriptor::new::<Request, ()>(key("test/twice")),
    ];

    let error = EntryTable::new(&entries, &topology()).expect_err("duplicate keys fail");

    assert!(matches!(error, EntryError::DuplicateKey { key } if key.as_str() == "test/twice"));
    assert_eq!(
        error.to_string(),
        "entry 'test/twice' is registered more than once"
    );
}

#[test]
fn entries_run_only_in_the_root_or_a_declared_scope() {
    let undeclared = [EntryDescriptor::new::<Undeclared, ()>(key(
        "test/undeclared",
    ))];
    let transient = [EntryDescriptor::new::<Transient, ()>(key("test/transient"))];

    let undeclared = EntryTable::new(&undeclared, &topology()).expect_err("undeclared fails");
    let transient = EntryTable::new(&transient, &topology()).expect_err("transient fails");

    assert!(matches!(
        undeclared,
        EntryError::UndeclaredScope {
            scope: UNDECLARED_ID,
            ..
        }
    ));
    assert_eq!(
        undeclared.to_string(),
        "entry 'test/undeclared' runs in scope 'test/entry-undeclared', which the active protocol does not open"
    );
    assert!(matches!(transient, EntryError::TransientScope { .. }));
}
