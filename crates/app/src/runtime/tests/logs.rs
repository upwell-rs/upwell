//! Log capture of instances opened through the prepared opener.

#![cfg(feature = "tracing-subscriber")]

use futures::StreamExt;
use tracing::Dispatch;
use tracing::dispatcher::DefaultGuard;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

use super::*;
use crate::logs::{ScopeLog, ScopeLogLayer, ScopeLogStream, ScopeLogs};

fn install() -> DefaultGuard {
    tracing::dispatcher::set_default(&Dispatch::new(
        Registry::default().with(ScopeLogLayer::new()),
    ))
}

async fn messages(stream: ScopeLogStream) -> Vec<String> {
    stream
        .filter_map(|item| async move {
            match item {
                ScopeLog::Record(record) => Some(record.message),
                _ => None,
            }
        })
        .collect()
        .await
}

#[tokio::test]
async fn a_run_streams_its_log_to_a_subscriber_taken_before_it_starts() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let run = sessions.builder(Session).run(|| async {
        tracing::info!("working");
        tokio::task::yield_now().await;
        tracing::info!("done");
    });
    let stream = run.logs();

    run.await.expect("the run succeeds");

    assert_eq!(messages(stream).await, ["working", "done"]);
}

#[tokio::test]
async fn a_run_nobody_watches_captures_nothing() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");

    let seen = sessions
        .builder(Session)
        .run(|logs: ScopeLogs| async move { logs.is_capturing() })
        .await
        .expect("the run succeeds");

    assert!(!seen);
}

#[tokio::test]
async fn the_work_subscribes_to_its_own_instance_through_scope_logs() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");

    let stream = sessions
        .builder(Session)
        .capture_logs()
        .run(|logs: ScopeLogs| async move {
            let stream = logs.subscribe();

            tracing::info!("inside");

            stream
        })
        .await
        .expect("the run succeeds");

    assert_eq!(messages(stream).await, ["inside"]);
}

#[tokio::test]
async fn an_open_instance_captures_when_asked() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let scope = sessions
        .builder(Session)
        .capture_logs()
        .open_default()
        .await
        .expect("the session opens");
    let stream = scope.logs();

    {
        let _entered = scope.span().entered();

        tracing::info!("in the session");
    }

    drop(scope);

    assert_eq!(messages(stream).await, ["in the session"]);
}

#[tokio::test]
async fn an_open_instance_that_does_not_capture_has_no_span_and_an_ended_stream() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let scope = open_session(&runtime).await;

    assert!(scope.span().is_disabled());
    assert!(messages(scope.logs()).await.is_empty());
}
