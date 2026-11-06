//! The `upwell.scope` span of every opened instance, capturing or not.

#![cfg(feature = "tracing-subscriber")]

use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use tracing::dispatcher::DefaultGuard;
use tracing::span::Attributes;
use tracing::{Dispatch, Id, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

use super::*;
use crate::logs::{SCOPE_SPAN_NAME, ScopeLog, ScopeLogLayer};
use crate::{EntryDescriptor, Opening};

/// Counts the `upwell.scope` spans created and entered.
#[derive(Default)]
struct SpanCounts {
    created: AtomicUsize,
    entered: AtomicUsize,
}

/// A layer feeding [`SpanCounts`].
struct Counting(Arc<SpanCounts>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Counting {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == SCOPE_SPAN_NAME {
            self.0.created.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let named = ctx
            .span(id)
            .is_some_and(|span| span.name() == SCOPE_SPAN_NAME);

        if named {
            self.0.entered.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn counted() -> (Arc<SpanCounts>, DefaultGuard) {
    let counts = Arc::new(SpanCounts::default());
    let dispatch = Dispatch::new(
        Registry::default()
            .with(ScopeLogLayer::new())
            .with(Counting(Arc::clone(&counts))),
    );

    (counts, tracing::dispatcher::set_default(&dispatch))
}

fn assert_spanned(counts: &SpanCounts) {
    assert_eq!(counts.created.load(Ordering::SeqCst), 1);
    assert!(counts.entered.load(Ordering::SeqCst) >= 1);
}

async fn runtime() -> AppRuntime {
    let entries = [
        EntryDescriptor::new::<Session, ()>(key("test/ping")),
        EntryDescriptor::new::<Session, (Visitor,)>(key("test/greet")),
    ];

    build_runtime_with_entries(visitor_destination(), &entries)
        .await
        .0
}

async fn open_for(
    runtime: &AppRuntime,
    entry: &'static str,
    capture: bool,
) -> crate::EntryScope<Session> {
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let entry = sessions.entry(key(entry)).expect("the entry resolves");
    let builder = sessions.builder(Session).seed(Visitor(1));
    let builder = if capture {
        builder.capture_logs()
    } else {
        builder
    };

    builder
        .open_when_needed(Some(&entry))
        .await
        .expect("the instance opens")
}

#[tokio::test]
async fn a_non_capturing_open_creates_and_enters_the_span() {
    let (counts, _guard) = counted();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");

    let scope = sessions
        .builder(Session)
        .open_default()
        .await
        .expect("the session opens");

    assert_spanned(&counts);
    assert!(!scope.span().is_disabled());
}

#[tokio::test]
async fn a_capturing_open_routes_a_dependency_crates_events_to_the_instance() {
    let (counts, _guard) = counted();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let scope = sessions
        .builder(Session)
        .capture_logs()
        .open_on_demand()
        .await
        .expect("the session opens");
    let stream = scope.logs();

    scope
        .span()
        .in_scope(|| tracing::info!(target: "dependency::crate", "from a dependency"));
    drop(scope);

    let records: Vec<_> = stream
        .filter_map(|item| async move {
            match item {
                ScopeLog::Record(record) => Some(record),
                _ => None,
            }
        })
        .collect()
        .await;

    assert_spanned(&counts);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].target, "dependency::crate");
}

#[tokio::test]
async fn open_with_bound_seed_creates_and_enters_the_span() {
    let (counts, _guard) = counted();
    let (runtime, _registry) = build_runtime(visitor_destination()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");

    sessions
        .builder(Session)
        .open_with_bound_seed(Opening::Everything, || Visitor(1))
        .await
        .expect("the session opens");

    assert_spanned(&counts);
}

#[tokio::test]
async fn an_immediate_open_when_needed_creates_and_enters_the_span() {
    let (counts, _guard) = counted();
    let runtime = runtime().await;

    let scope = open_for(&runtime, "test/greet", false).await;

    assert!(scope.is_open());
    assert_spanned(&counts);
}

#[tokio::test]
async fn a_deferred_open_when_needed_creates_the_span_and_enters_it_when_it_opens() {
    let (counts, _guard) = counted();
    let runtime = runtime().await;

    let scope = open_for(&runtime, "test/ping", false).await;

    assert!(!scope.is_open());
    assert_eq!(counts.created.load(Ordering::SeqCst), 1);
    assert_eq!(counts.entered.load(Ordering::SeqCst), 0);

    scope.container().await.expect("the scope opens");

    assert_spanned(&counts);
}

#[tokio::test]
async fn a_capturing_deferred_open_when_needed_captures_events() {
    let (_counts, _guard) = counted();
    let runtime = runtime().await;
    let scope = open_for(&runtime, "test/ping", true).await;
    let stream = scope.logs();

    scope.span().in_scope(|| tracing::info!("deferred"));
    drop(scope);

    let count = stream.count().await;

    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_run_creates_and_enters_the_span_without_capture() {
    let (counts, _guard) = counted();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");

    sessions
        .builder(Session)
        .run(|| async {})
        .await
        .expect("the run succeeds");

    assert_spanned(&counts);
}
