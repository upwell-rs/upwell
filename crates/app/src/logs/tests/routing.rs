use super::*;

#[tokio::test]
async fn parallel_instances_each_capture_only_their_events() {
    let _guard = install();
    let (first, first_stream) = instance(1, &options());
    let (second, second_stream) = instance(2, &options());

    let one = async {
        tracing::info!("first a");
        tokio::task::yield_now().await;
        tracing::info!(target: "dependency::crate", "first b");
    }
    .instrument(first.span().clone());
    let two = async {
        tracing::info!("second a");
        tokio::task::yield_now().await;
        tracing::warn!("second b");
    }
    .instrument(second.span().clone());

    tokio::join!(one, two);
    first.finish();
    second.finish();

    let first = records(first_stream).await;
    let second = records(second_stream).await;

    assert_eq!(messages(&first), ["first a", "first b"]);
    assert_eq!(first[1].target, "dependency::crate");
    assert_eq!(messages(&second), ["second a", "second b"]);
}

#[tokio::test]
async fn a_plainly_spawned_task_reaches_no_instance() {
    let _guard = install();
    let (logs, stream) = instance(1, &options());

    async {
        tokio::spawn(async { tracing::info!("lost") })
            .await
            .expect("the task completes");
        tracing::info!("kept");
    }
    .instrument(logs.span().clone())
    .await;
    logs.finish();

    assert_eq!(messages(&records(stream).await), ["kept"]);
}

#[tokio::test]
async fn a_nested_instance_delivers_to_itself_only() {
    let _guard = install();
    let (outer, outer_stream) = instance(1, &options());

    let inner_stream = async {
        tracing::info!("outer before");

        let (inner, inner_stream) = instance(2, &options());

        async { tracing::info!("inner") }
            .instrument(inner.span().clone())
            .await;
        inner.finish();
        tracing::info!("outer after");

        inner_stream
    }
    .instrument(outer.span().clone())
    .await;

    outer.finish();

    assert_eq!(
        messages(&records(outer_stream).await),
        ["outer before", "outer after"]
    );
    assert_eq!(messages(&records(inner_stream).await), ["inner"]);
}

#[tokio::test]
async fn a_non_capturing_instance_passes_events_to_the_enclosing_capturer() {
    let _guard = install();
    let (outer, outer_stream) = instance(1, &options());

    async {
        let inner = InstanceLogs::open(&customer(2), &LogOptions::new(), None);

        async { tracing::info!("inner") }
            .instrument(inner.span().clone())
            .await;
    }
    .instrument(outer.span().clone())
    .await;
    outer.finish();

    assert_eq!(messages(&records(outer_stream).await), ["inner"]);
}

#[tokio::test]
async fn records_carry_the_loggable_params_and_never_the_rest() {
    let _guard = install();
    let (logs, stream) = instance(7, &options());

    async { tracing::info!(attempt = 3, label = "x", "hello") }
        .instrument(logs.span().clone())
        .await;
    logs.finish();

    let records = records(stream).await;
    let record = &records[0];

    assert_eq!(record.scope, CUSTOMER_ID);
    assert_eq!(
        record.params.as_ref(),
        [LogField::new("id", "7".to_owned())]
    );
    assert_eq!(
        record.fields,
        [
            LogField::new("attempt", "3".to_owned()),
            LogField::new("label", "x".to_owned())
        ]
    );
    assert!(!format!("{record:?}").contains("secret"));
    assert_eq!(customer(7).token, "secret");
}

#[test]
fn the_span_renders_its_params() {
    let rendered = render::Rendered(&customer(9)).to_string();

    assert_eq!(rendered, "id=9");
}

#[tokio::test]
async fn a_capture_level_narrows_what_the_instance_keeps() {
    let _guard = install();
    let narrow = LogOptions {
        level: Some(LevelFilter::WARN),
        ..options()
    };
    let (logs, stream) = instance(1, &narrow);

    async {
        tracing::info!("dropped");
        tracing::error!("kept");
    }
    .instrument(logs.span().clone())
    .await;
    logs.finish();

    assert_eq!(messages(&records(stream).await), ["kept"]);
}

#[tokio::test]
async fn an_instance_without_a_layer_is_not_attached() {
    let logs = InstanceLogs::open(&customer(1), &options(), None);

    assert!(!logs.sink().expect("the instance captures").is_attached());
}

#[tokio::test]
async fn an_instance_with_the_layer_is_attached() {
    let _guard = install();
    let logs = InstanceLogs::open(&customer(1), &options(), None);

    assert!(logs.sink().expect("the instance captures").is_attached());
}
