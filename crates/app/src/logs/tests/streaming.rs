use futures::Stream;

use super::*;

#[tokio::test]
async fn events_before_the_first_subscriber_are_not_captured() {
    let _guard = install();
    let logs = InstanceLogs::open(&customer(1), &options(), None);
    let sink = logs.sink().expect("the instance captures");

    async { tracing::info!("early") }
        .instrument(logs.span().clone())
        .await;

    let stream = sink.subscribe();

    async { tracing::info!("late") }
        .instrument(logs.span().clone())
        .await;
    logs.finish();

    assert_eq!(messages(&records(stream).await), ["late"]);
}

#[tokio::test]
async fn a_slow_subscriber_lags_without_blocking_the_instance() {
    let _guard = install();
    let small = LogOptions {
        buffer: NonZeroUsize::new(2).expect("two is not zero"),
        ..options()
    };
    let (logs, mut stream) = instance(1, &small);

    async {
        for index in 0..5 {
            tracing::info!(index, "event");
        }
    }
    .instrument(logs.span().clone())
    .await;
    logs.finish();

    let mut items = Vec::new();

    while let Some(item) = stream.next().await {
        items.push(item);
    }

    assert!(matches!(items[0], ScopeLog::Record(_)));
    assert!(matches!(items[1], ScopeLog::Record(_)));
    assert!(matches!(items[2], ScopeLog::Lagged(3)));
    assert_eq!(items.len(), 3);
}

#[tokio::test]
async fn every_subscriber_receives_every_record() {
    let _guard = install();
    let (logs, first) = instance(1, &options());
    let second = logs.sink().expect("the instance captures").subscribe();

    async { tracing::info!("one") }
        .instrument(logs.span().clone())
        .await;
    logs.finish();

    assert_eq!(messages(&records(first).await), ["one"]);
    assert_eq!(messages(&records(second).await), ["one"]);
}

#[tokio::test]
async fn a_stream_of_a_closed_instance_ends_at_once() {
    let _guard = install();
    let (logs, _first) = instance(1, &options());
    let sink = Arc::clone(logs.sink().expect("the instance captures"));

    logs.finish();

    assert!(records(sink.subscribe()).await.is_empty());
}

#[tokio::test]
async fn dropping_the_instance_ends_its_streams() {
    let _guard = install();
    let (logs, stream) = instance(1, &options());

    drop(logs);

    assert!(records(stream).await.is_empty());
}

#[tokio::test]
async fn the_largest_buffer_is_clamped_instead_of_panicking() {
    let _guard = install();
    let huge = LogOptions {
        buffer: NonZeroUsize::MAX,
        ..options()
    };
    let (logs, stream) = instance(1, &huge);

    async { tracing::info!("one") }
        .instrument(logs.span().clone())
        .await;
    logs.finish();

    assert_eq!(messages(&records(stream).await), ["one"]);
}

#[tokio::test]
async fn counting_a_drop_wakes_a_pending_reader() {
    let (_sender, receiver) = tokio::sync::mpsc::channel(1);
    let lag = Arc::new(stream::LagCounter::default());
    let mut reader = ScopeLogStream::new(receiver, Arc::clone(&lag));
    let woken = Arc::new(WakeFlag::default());
    let waker = futures::task::waker(Arc::clone(&woken));
    let mut cx = std::task::Context::from_waker(&waker);

    assert!(
        std::pin::Pin::new(&mut reader)
            .poll_next(&mut cx)
            .is_pending()
    );

    lag.record_drop();

    assert!(woken.0.load(std::sync::atomic::Ordering::Acquire));
    assert!(matches!(
        std::pin::Pin::new(&mut reader).poll_next(&mut cx),
        std::task::Poll::Ready(Some(ScopeLog::Lagged(1)))
    ));
}

/// Remembers that it was woken.
#[derive(Default)]
struct WakeFlag(std::sync::atomic::AtomicBool);

impl futures::task::ArcWake for WakeFlag {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}
