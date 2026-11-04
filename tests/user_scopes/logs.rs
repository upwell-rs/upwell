//! Scope logs: opt-in `#[log]` params and per-instance capture through `ScopeBuilder::run`.

use futures::StreamExt;
use std::sync::Arc;
use tracing::Dispatch;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

use upwell::logs::{LogField, ScopeLog, ScopeLogLayer, ScopeLogs};
use upwell::scope;
use upwell::scope::Root;
use upwell::{ScopeOpener, component, methods};

use super::app;

/// A run whose account is loggable and whose token is not.
#[scope(parents = [Root])]
pub struct AuditRun {
    #[log]
    account: u64,
    #[allow(dead_code)]
    token: String,
}

/// Records whether the instance that built it captures logs, as seen while it was built.
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
async fn a_component_built_while_opening_sees_its_own_instance_sink() {
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
async fn a_nested_non_capturing_open_does_not_inherit_the_outer_sink() {
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

    assert!(!inner_capturing);
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
