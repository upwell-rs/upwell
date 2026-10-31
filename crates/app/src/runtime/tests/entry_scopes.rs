//! Scopes opened for an entry: deferred while the entry's plan builds nothing.

use super::*;
use crate::{EntryDescriptor, EntryScope};

/// A runtime whose session scope has a `Visitor` seed slot, an entry injecting nothing
/// (`test/ping`) and an entry injecting the visitor (`test/greet`).
async fn runtime() -> AppRuntime {
    let entries = [
        EntryDescriptor::new::<Session, ()>(key("test/ping")),
        EntryDescriptor::new::<Session, (Visitor,)>(key("test/greet")),
    ];
    let (runtime, _) = build_runtime_with_entries(visitor_destination(), &entries).await;

    runtime
}

/// Opens a session for `entry`, seeded with `seeds` visitors.
async fn open_for(
    runtime: &AppRuntime,
    entry: &'static str,
    seeds: &[u8],
) -> crate::Result<EntryScope<Session>> {
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let entry = sessions.entry(key(entry)).expect("the entry resolves");
    let builder = seeds.iter().fold(sessions.builder(), |builder, seed| {
        builder.seed(Visitor(*seed))
    });

    builder.open_when_needed(&entry).await
}

#[tokio::test]
async fn an_entry_planning_nothing_opens_its_scope_when_first_resolved() {
    let runtime = runtime().await;

    let session = open_for(&runtime, "test/ping", &[3])
        .await
        .expect("an entry planning nothing defers its scope");

    assert!(!session.is_open());
    assert_eq!(
        session.current().scope(),
        Singleton::KEY,
        "reads see the scope the instance opens under"
    );

    let visitor = session
        .extract::<Visitor>()
        .await
        .expect("resolving opens the scope with its seeds");

    assert!(session.is_open());
    assert_eq!(session.current().scope(), Session::KEY);
    assert_eq!(visitor, Visitor(3));
}

#[tokio::test]
async fn an_entry_planning_something_opens_its_scope_at_once() {
    let runtime = runtime().await;

    let session = open_for(&runtime, "test/greet", &[5])
        .await
        .expect("the scope opens");

    assert!(session.is_open());
    assert_eq!(session.current().scope(), Session::KEY);
}

#[tokio::test]
async fn a_deferred_scope_opens_on_the_generation_it_was_planned_on() {
    let runtime = runtime().await;
    let session = open_for(&runtime, "test/ping", &[])
        .await
        .expect("an entry planning nothing defers its scope");

    let committed = publish_next(&runtime).await;
    let opened = session
        .container()
        .await
        .expect("the deferred scope opens after a publication");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        runtime.scope_generation(opened),
        Some(RuntimeGenerationId::INITIAL)
    );
}

#[tokio::test]
async fn a_deferred_scope_that_failed_to_open_fails_again() {
    let runtime = runtime().await;
    let session = open_for(&runtime, "test/ping", &[1, 2])
        .await
        .expect("seeds are checked when the scope opens");

    let first = expect_app_error(
        session.container().await,
        "two seeds for one slot were accepted",
    );
    let second = expect_app_error(
        session.container().await,
        "a failed deferred scope opened later",
    );

    assert!(matches!(
        first,
        Error::Di(upwell_di::Error::DuplicateSeed { .. })
    ));
    assert!(matches!(
        second,
        Error::DeferredScopeFailed { scope: SESSION_ID }
    ));
    assert!(!session.is_open());
}
