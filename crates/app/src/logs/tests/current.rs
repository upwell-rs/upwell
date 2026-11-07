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
