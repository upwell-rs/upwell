//! Scope logs inside a run under subscribers whose scope log layer filters out its routing
//! span, or that never saw it.

#![cfg(feature = "tracing-subscriber")]

use tracing::Dispatch;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

use super::logs::{install, messages};
use super::*;
use crate::logs::ScopeLogs;

/// A subscriber whose scope log layer admits application events but not routing spans.
fn scope_off() -> Dispatch {
    let layer = crate::builtins::logging::scope_log_layer(
        &crate::builtins::config::LoggingConfig::default(),
        Some(std::ffi::OsStr::new("myapp=info,upwell::scope=off")),
    )
    .expect("filter directives are valid");

    Dispatch::new(Registry::default().with(layer))
}

fn sessions(runtime: &AppRuntime) -> ScopeOpener<Session> {
    runtime.opener::<Session>().expect("session is declared")
}

#[tokio::test]
async fn an_explicit_scope_off_disables_capture_inside_a_run() {
    let _guard = tracing::dispatcher::set_default(&scope_off());
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let run = opener.builder(Session).capture_logs().run(|| async {
        tracing::info!(target: "myapp", "inside the run");

        ScopeLogs::current().is_capturing()
    });
    let stream = run.logs();

    let capturing = run.await.expect("the run succeeds");

    assert!(
        !capturing,
        "ScopeLogs reports no capture under upwell::scope=off"
    );
    assert!(messages(stream).await.is_empty());
}

#[tokio::test]
async fn a_subscriber_that_disables_scope_capture_never_feeds_a_run_captured_under_another() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let other = scope_off();
    let run = opener.builder(Session).capture_logs().run(|| async move {
        tracing::info!(target: "myapp", "kept");
        tracing::dispatcher::with_default(&other, || {
            tracing::info!(target: "myapp", "under the other subscriber");
        });
    });
    let stream = run.logs();

    run.await.expect("the run succeeds");

    assert_eq!(messages(stream).await, ["kept"]);
}

#[tokio::test]
async fn a_run_captured_under_one_subscriber_is_not_current_under_another() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let other = scope_off();
    let run = opener.builder(Session).capture_logs().run(|| async move {
        let inside = ScopeLogs::current().is_capturing();
        let other =
            tracing::dispatcher::with_default(&other, || ScopeLogs::current().is_capturing());

        (inside, other)
    });

    let (inside, other) = run.await.expect("the run succeeds");

    assert!(inside, "the run's own subscriber shows its log");
    assert!(
        !other,
        "a subscriber whose layer never took the run's span shows no log"
    );
}
