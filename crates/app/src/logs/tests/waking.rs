use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Wake, Waker};
use std::thread;
use std::time::Duration;

use futures::Stream;

use super::*;

/// A waker that subscribes to its sink, as a reader woken by a record might.
struct Resubscribe {
    sink: Arc<ScopeLogSink>,
}

impl Wake for Resubscribe {
    fn wake(self: Arc<Self>) {
        drop(self.sink.subscribe());
    }
}

/// Runs `scenario` on its own thread, which fails the test when it does not finish: a sink that
/// wakes readers under its lock deadlocks there.
fn finishes(scenario: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();

    thread::spawn(move || {
        scenario();
        done.send(()).expect("the test waits for the scenario");
    });

    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("delivering to a reader that resubscribes when woken does not deadlock");
}

#[test]
fn a_reader_woken_by_a_record_may_subscribe() {
    finishes(|| {
        let _guard = install();
        let (logs, mut stream) = instance(1, &options());
        let sink = Arc::clone(logs.sink().expect("the instance captures"));
        let waker = Waker::from(Arc::new(Resubscribe { sink }));
        let mut cx = Context::from_waker(&waker);

        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());

        let _entered = logs.span().enter();

        tracing::info!("wakes the reader");
    });
}

#[test]
fn a_reader_woken_by_a_lag_report_may_subscribe() {
    finishes(|| {
        let _guard = install();
        let small = LogOptions {
            buffer: NonZeroUsize::new(1).expect("one is not zero"),
            ..options()
        };
        let (logs, mut stream) = instance(1, &small);
        let sink = Arc::clone(logs.sink().expect("the instance captures"));
        let waker = Waker::from(Arc::new(Resubscribe { sink }));
        let mut cx = Context::from_waker(&waker);
        let entered = logs.span().enter();

        tracing::info!("first");

        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_ready());

        tracing::info!("buffered");
        tracing::info!("dropped");

        drop(entered);
    });
}
