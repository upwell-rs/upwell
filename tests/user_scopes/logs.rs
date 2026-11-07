//! Scope logs: opt-in `#[log]` params and per-instance capture through `ScopeBuilder::run`.

use futures::StreamExt;
use std::sync::Arc;
use tracing::Dispatch;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

use upwell::logs::{LogField, ScopeLog, ScopeLogLayer, ScopeLogStream, ScopeLogs};
use upwell::scope;
use upwell::scope::Root;
use upwell::{ScopeOpener, component, methods};
use upwell_app::HookTarget;
use upwell_di::context::{InvocationContext, InvocationTarget, TargetRef};

use super::app;

/// A run whose account is loggable and whose token is not.
#[scope(parents = [Root])]
pub struct AuditRun {
    #[log]
    account: u64,
    #[allow(dead_code)]
    token: String,
}

/// Records whether events made where it was built are captured. `ScopeLogs` resolves like event
/// delivery, so this pins what code running during an instance's own open sees, not a component
/// property.
#[component(scope = AuditRun, default_factory = false)]
struct CaptureProbe {
    capturing: bool,
}

#[methods]
impl CaptureProbe {
    #[init]
    fn new(logs: ScopeLogs, _run: Arc<AuditRun>) -> Self {
        Self {
            capturing: logs.is_capturing(),
        }
    }
}

/// Logs from its constructor and records whether `ScopeLogs` captured there.
#[component(scope = AuditRun, default_factory = false)]
struct LoudProbe {
    capturing: bool,
}

#[methods]
impl LoudProbe {
    #[init]
    fn new(logs: ScopeLogs, _run: Arc<AuditRun>) -> Self {
        tracing::info!("constructing");

        Self {
            capturing: logs.is_capturing(),
        }
    }
}

/// A target standing in for a job run.
#[derive(Debug)]
struct Job;

impl InvocationTarget for Job {}

fn install() -> tracing::dispatcher::DefaultGuard {
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

/// Runs a capturing audit whose work injects a [`LoudProbe`] inside `context`, returning
/// whether the probe's constructor saw a capture and what the run captured.
async fn probed_run_in(
    audits: &ScopeOpener<AuditRun>,
    context: InvocationContext,
) -> (bool, Vec<String>) {
    context
        .enter(async {
            let run = audits
                .builder(audit(1))
                .capture_logs()
                .run(|probe: Arc<LoudProbe>| async move { probe.capturing });
            let stream = run.logs();
            let capturing = run.await.expect("the run succeeds");

            (capturing, messages(stream).await)
        })
        .await
}

fn audit(account: u64) -> AuditRun {
    AuditRun {
        account,
        token: "secret".to_owned(),
    }
}

async fn probe_of(scope: &upwell::OpenScope<AuditRun>) -> bool {
    scope
        .container()
        .extract::<Arc<CaptureProbe>>()
        .await
        .expect("the probe builds")
        .capturing
}

#[tokio::test]
async fn construction_during_an_open_sees_the_opening_instances_sink() {
    let _guard = install();
    let app = app("user-scopes-open-sink").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");
    let captured = audits
        .builder(audit(1))
        .capture_logs()
        .open_default()
        .await
        .expect("opens");
    let plain = audits
        .builder(audit(2))
        .open_default()
        .await
        .expect("opens");

    assert!(probe_of(&captured).await);
    assert!(!probe_of(&plain).await);
}

#[tokio::test]
async fn a_non_capturing_open_sees_the_enclosing_runs_capture() {
    let _guard = install();
    let app = app("user-scopes-open-nested").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");

    let inner_capturing = audits
        .builder(audit(1))
        .capture_logs()
        .run(|opener: ScopeOpener<AuditRun>| async move {
            let inner = opener
                .builder(audit(2))
                .open_default()
                .await
                .expect("the inner opens");

            probe_of(&inner).await
        })
        .await
        .expect("the run succeeds");

    assert!(
        inner_capturing,
        "the inner open's events reach the run's capture"
    );
}

#[tokio::test]
async fn a_non_capturing_open_when_needed_sees_the_enclosing_runs_capture() {
    let _guard = install();
    let app = app("user-scopes-open-when-needed").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");

    let inner_capturing = audits
        .builder(audit(1))
        .capture_logs()
        .run(|opener: ScopeOpener<AuditRun>| async move {
            let inner = opener
                .builder(audit(2))
                .open_when_needed(None)
                .await
                .expect("the inner opens");
            let probe = inner
                .container()
                .await
                .expect("the container opens")
                .extract::<Arc<CaptureProbe>>()
                .await
                .expect("the probe builds");

            probe.capturing
        })
        .await
        .expect("the run succeeds");

    assert!(
        inner_capturing,
        "the inner open's events reach the run's capture"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runs_capture_their_own_events_and_loggable_params() {
    let dispatch = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let app = app("user-scopes-logs").await;
    let audits = app
        .runtime()
        .opener::<AuditRun>()
        .expect("discovery declares the audit scope");

    let runs = (0..16_u64).map(|account| {
        let audits = audits.clone();
        let dispatch = dispatch.clone();

        tokio::spawn(
            async move {
                let run = audits
                    .builder(AuditRun {
                        account,
                        token: "secret".to_owned(),
                    })
                    .run(move || async move {
                        tracing::info!(account, "start");
                        tokio::task::yield_now().await;
                        tracing::info!(target: "dependency::crate", account, "finish");
                    });
                let logs = run.logs();

                run.await.expect("the run succeeds");

                (account, logs.collect::<Vec<_>>().await)
            }
            .with_subscriber(dispatch),
        )
    });

    for outcome in futures::future::join_all(runs).await {
        let (account, items) = outcome.expect("the run task completes");
        let records: Vec<_> = items
            .into_iter()
            .map(|item| match item {
                ScopeLog::Record(record) => record,
                other => panic!("unexpected item {other:?}"),
            })
            .collect();

        assert_eq!(records.len(), 2);

        for record in &records {
            assert_eq!(
                record.params.as_ref(),
                [LogField::new("account", account.to_string())]
            );
            assert!(
                record
                    .fields
                    .contains(&LogField::new("account", account.to_string()))
            );
        }

        assert_eq!(records[1].target, "dependency::crate");
    }
}

#[tokio::test]
async fn a_run_started_by_a_job_captures_its_constructors() {
    let _guard = install();
    let app = app("user-scopes-job-run").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");
    let job = audits.context(TargetRef::from_static(&Job));

    let (capturing, messages) = probed_run_in(&audits, job).await;

    assert!(capturing, "the constructor sees the run's capture");
    assert!(messages.iter().any(|message| message == "constructing"));
}

#[tokio::test]
async fn a_run_started_by_a_lifecycle_hook_captures_its_constructors() {
    static STARTUP: HookTarget = HookTarget { kind: "startup" };

    let _guard = install();
    let app = app("user-scopes-hook-run").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");
    let hook = audits.context(TargetRef::from_static(&STARTUP));

    let (capturing, messages) = probed_run_in(&audits, hook).await;

    assert!(capturing, "the constructor sees the run's capture");
    assert!(messages.iter().any(|message| message == "constructing"));
}

#[tokio::test]
async fn a_run_started_inside_another_run_keeps_its_construction_events() {
    let _guard = install();
    let app = app("user-scopes-nested-run").await;
    let audits = app.runtime().opener::<AuditRun>().expect("declared");
    let outer =
        audits
            .builder(audit(1))
            .capture_logs()
            .run(|opener: ScopeOpener<AuditRun>| async move {
                let inner = opener
                    .builder(audit(2))
                    .capture_logs()
                    .run(|probe: Arc<LoudProbe>| async move { probe.capturing });
                let inner_stream = inner.logs();
                let capturing = inner.await.expect("the inner run succeeds");

                (capturing, inner_stream)
            });
    let outer_stream = outer.logs();

    let (capturing, inner_stream) = outer.await.expect("the outer run succeeds");

    assert!(
        capturing,
        "the inner constructor sees the inner run's capture"
    );
    assert!(
        messages(inner_stream)
            .await
            .iter()
            .any(|message| message == "constructing")
    );
    assert!(
        !messages(outer_stream)
            .await
            .iter()
            .any(|message| message == "constructing"),
        "the outer run never sees the inner run's construction"
    );
}
