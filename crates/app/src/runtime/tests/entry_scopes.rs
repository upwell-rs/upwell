//! Scopes opened for an entry: deferred while the entry's plan builds nothing.

use super::*;
use crate::{EntryDescriptor, EntryScope, Opening};

/// A runtime whose session scope has a `Visitor` seed slot, an entry injecting nothing
/// (`test/ping`), an entry injecting the visitor (`test/greet`) and an entry injecting the root's
/// hook manager (`test/hooks`).
async fn runtime() -> AppRuntime {
    let entries = [
        EntryDescriptor::new::<Session, ()>(key("test/ping")),
        EntryDescriptor::new::<Session, (Visitor,)>(key("test/greet")),
        EntryDescriptor::new::<Session, (HookManager,)>(key("test/hooks")),
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

    builder.open_when_needed(Some(&entry)).await
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
async fn a_singleton_target_is_read_without_opening_a_deferred_scope() {
    let runtime = runtime().await;

    let session = open_for(&runtime, "test/hooks", &[])
        .await
        .expect("an entry injecting only a singleton defers its scope");

    assert!(
        session.read_planned::<HookManager>().is_some(),
        "the plan pre-resolved the singleton"
    );

    session
        .extract::<HookManager>()
        .await
        .expect("the singleton extracts");

    assert!(
        !session.is_open(),
        "reading a planned singleton opened the deferred scope"
    );
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

/// Opens a session as `opening` says with a bound visitor seed, and reports whether the opening
/// asked for the seed.
async fn open_bound(
    runtime: &AppRuntime,
    entry: Option<&'static str>,
) -> (OpenScope<Session>, bool) {
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let entry = entry.map(|entry| sessions.entry(key(entry)).expect("the entry resolves"));
    let opening = entry.as_ref().map_or(Opening::Everything, Opening::Entry);
    let mut asked = false;
    let session = sessions
        .builder()
        .open_with_bound_seed(opening, || {
            asked = true;

            Visitor(5)
        })
        .await
        .expect("the session opens");

    (session, asked)
}

#[tokio::test]
async fn an_opening_asks_for_a_bound_seed_only_when_it_reads_it() {
    let runtime = runtime().await;

    let (greet, greet_asked) = open_bound(&runtime, Some("test/greet")).await;
    let (ping, ping_asked) = open_bound(&runtime, Some("test/ping")).await;
    let (_, default_asked) = open_bound(&runtime, None).await;

    assert!(greet_asked, "the entry injects the seed");
    assert!(!ping_asked, "the entry injects nothing");
    assert!(!default_asked, "no factory of the scope reads the seed");
    assert_eq!(
        greet.container().extract::<Visitor>().await.ok(),
        Some(Visitor(5))
    );
    assert!(
        ping.container().extract::<Visitor>().await.is_err(),
        "a seed the opening left out is empty until it is filled"
    );

    ping.container()
        .ensure_seed(|| Visitor(6))
        .expect("the slot is a seed");

    assert_eq!(
        ping.container().extract::<Visitor>().await.ok(),
        Some(Visitor(6))
    );
}
