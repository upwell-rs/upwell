//! `ScopeLogs::current()` read from inside subscriber callbacks.

use std::sync::atomic::{AtomicBool, Ordering};

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
