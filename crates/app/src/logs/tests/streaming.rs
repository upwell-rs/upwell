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
        buffer: 2,
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
