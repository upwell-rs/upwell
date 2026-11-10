//! `ScopeLogs::current()` read from inside subscriber callbacks.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::span::{Attributes, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Id, Metadata, Subscriber};
use tracing_subscriber::registry::LookupSpan;

use super::*;

/// A value whose `Debug` rendering asks for the current scope log, as a user formatter may.
struct AsksForTheLog(Arc<AtomicBool>);

impl std::fmt::Debug for AsksForTheLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0
            .store(ScopeLogs::current().is_capturing(), Ordering::SeqCst);

        formatter.write_str("asked")
    }
}

#[test]
fn reading_the_log_while_a_formatter_records_on_the_scope_span_does_not_deadlock() {
    let dispatch = Dispatch::new(
        Registry::default()
            .with(ScopeLogLayer::new())
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink)),
    );
    let (done, finished) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let seen = Arc::new(AtomicBool::new(false));

        tracing::dispatcher::with_default(&dispatch, || {
            let (logs, _stream) = instance(1, &options());

            logs.span().in_scope(|| {
                logs.span().record(
                    "params",
                    tracing::field::debug(AsksForTheLog(Arc::clone(&seen))),
                );
            });
            logs.finish();
        });

        done.send(seen.load(Ordering::SeqCst))
            .expect("the test waits");
    });

    let seen = finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("reading the log inside a formatter neither deadlocks nor panics");

    assert!(seen, "the formatter sees the instance's log");
}

/// A [`LookupSpan`] subscriber that is not a [`Registry`] and does not expose one, as an
/// application's own span store may be. It keeps `Subscriber::current_span`'s default, so its
/// events name their parent explicitly.
struct OwnStore(Registry);

impl Subscriber for OwnStore {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.0.register_callsite(metadata)
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.0.enabled(metadata)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        self.0.max_level_hint()
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        self.0.new_span(attributes)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        self.0.record(span, values);
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.0.record_follows_from(span, follows);
    }

    fn event(&self, event: &Event<'_>) {
        self.0.event(event);
    }

    fn enter(&self, span: &Id) {
        self.0.enter(span);
    }

    fn exit(&self, span: &Id) {
        self.0.exit(span);
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.0.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.0.try_close(id)
    }
}

impl<'a> LookupSpan<'a> for OwnStore {
    type Data = <Registry as LookupSpan<'a>>::Data;

    fn span_data(&'a self, id: &Id) -> Option<Self::Data> {
        self.0.span_data(id)
    }
}

#[test]
fn the_log_is_current_under_any_span_store_the_layer_supports() {
    let dispatch = Dispatch::new(OwnStore(Registry::default()).with(ScopeLogLayer::new()));

    let (capturing, captured) = tracing::dispatcher::with_default(&dispatch, || {
        let (logs, stream) = instance(1, &options());
        let capturing = logs.span().in_scope(|| {
            tracing::info!(parent: logs.span(), "in the instance");

            ScopeLogs::current().is_capturing()
        });

        logs.finish();

        (capturing, futures::executor::block_on(records(stream)))
    });

    assert!(capturing, "ScopeLogs finds the instance's log");
    assert_eq!(messages(&captured), ["in the instance"]);
}

#[test]
fn a_detached_span_entered_inside_a_capture_hides_the_capture_from_the_current_log() {
    let _guard = install();
    let (logs, _stream) = instance(1, &options());

    let (inside, detached) = logs.span().in_scope(|| {
        let inside = ScopeLogs::current().is_capturing();
        let detached = tracing::error_span!(parent: None, "detached")
            .in_scope(|| ScopeLogs::current().is_capturing());

        (inside, detached)
    });

    logs.finish();

    assert!(inside, "the instance's own span shows its log");
    assert!(
        !detached,
        "a span with no capturing ancestor shows no log, as its events reach none"
    );
}

#[test]
fn a_capture_entered_under_one_subscriber_is_not_current_under_another() {
    let _guard = install();
    let without_layer = Dispatch::new(Registry::default());
    let with_own_layer = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let (logs, _stream) = instance(1, &options());

    let (inside, without, own) = logs.span().in_scope(|| {
        let inside = ScopeLogs::current().is_capturing();
        let without = tracing::dispatcher::with_default(&without_layer, || {
            ScopeLogs::current().is_capturing()
        });
        let own = tracing::dispatcher::with_default(&with_own_layer, || {
            ScopeLogs::current().is_capturing()
        });

        (inside, without, own)
    });

    logs.finish();

    assert!(inside, "the subscriber that took the span shows its log");
    assert!(
        !without,
        "a subscriber without a scope log layer shows no log, as its events reach none"
    );
    assert!(
        !own,
        "another subscriber's scope log layer shows no log it did not take"
    );
}

#[test]
fn a_formatter_rendering_an_event_inside_a_capture_sees_the_log() {
    let dispatch = Dispatch::new(
        Registry::default()
            .with(ScopeLogLayer::new())
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink)),
    );
    let seen = Arc::new(AtomicBool::new(false));

    tracing::dispatcher::with_default(&dispatch, || {
        let (logs, _stream) = instance(1, &options());

        logs.span().in_scope(|| {
            tracing::info!(value = ?AsksForTheLog(Arc::clone(&seen)), "rendered");
        });
        logs.finish();
    });

    assert!(
        seen.load(Ordering::SeqCst),
        "a formatter rendering an event made in the instance sees its log"
    );
}

#[test]
fn another_subscribers_exit_keeps_the_current_capture() {
    let capturing = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let other = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let foreign = tracing::dispatcher::with_default(&other, || tracing::error_span!("foreign"));
    let entered_foreign = foreign.enter();

    let (before, after) = tracing::dispatcher::with_default(&capturing, || {
        let (logs, _stream) = instance(1, &options());
        let _entered = logs.span().enter();
        let before = ScopeLogs::current().is_capturing();

        drop(entered_foreign);

        let after = ScopeLogs::current().is_capturing();

        (before, after)
    });

    assert!(before, "the instance's span shows its log");
    assert!(
        after,
        "another subscriber's span exiting with the same id leaves the instance's entry"
    );
}

#[test]
fn another_subscribers_span_entered_inside_a_capture_does_not_hide_it() {
    let capturing = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let other = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));

    let (inside, under_other) = tracing::dispatcher::with_default(&capturing, || {
        let (logs, _stream) = instance(1, &options());

        logs.span().in_scope(|| {
            let foreign =
                tracing::dispatcher::with_default(&other, || tracing::error_span!("foreign"));

            foreign.in_scope(|| {
                let under_other = tracing::dispatcher::with_default(&other, || {
                    ScopeLogs::current().is_capturing()
                });

                (ScopeLogs::current().is_capturing(), under_other)
            })
        })
    });

    assert!(
        inside,
        "a sinkless span of another subscriber hides no capture of the current one"
    );
    assert!(
        !under_other,
        "the other subscriber shows no log it did not take"
    );
}
