//! Parallel instances of one scope share no scoped components.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{CustomerClient, CustomerSync, SyncAudit, UNUSED_BUILDS, app, ok, opener};

/// How many instances [`parallel_runs_never_share_scoped_components`] runs at once.
const RUNS: u64 = 64;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runs_never_share_scoped_components() {
    let app = app("user-scopes-isolation").await;
    let sync = opener(&app);
    let runs = (0..RUNS).map(|customer| {
        let sync = sync.clone();

        tokio::spawn(async move {
            sync.builder(CustomerSync { customer })
                .try_run(
                    |client: Arc<CustomerClient>, audit: Arc<SyncAudit>| async move {
                        tokio::task::yield_now().await;

                        ok((customer, client, audit))
                    },
                )
                .await
                .expect("the run succeeds")
        })
    });
    let outcomes = futures::future::join_all(runs).await;
    let mut clients = HashSet::new();

    for outcome in outcomes {
        let (customer, client, audit) = outcome.expect("the run task completes");

        assert_eq!(client.customer(), customer);
        assert!(Arc::ptr_eq(&audit.client, &client));
        assert!(clients.insert(Arc::as_ptr(&client) as usize));
    }

    assert_eq!(clients.len(), RUNS as usize);
    assert_eq!(UNUSED_BUILDS.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_run_closes_its_instance_when_the_work_finishes() {
    let app = app("user-scopes-close").await;
    let client = opener(&app)
        .builder(CustomerSync { customer: 1 })
        .try_run(|client: Arc<CustomerClient>| async move { ok(Arc::downgrade(&client)) })
        .await
        .expect("the run succeeds");

    assert!(client.upgrade().is_none());
}

#[tokio::test]
async fn sequential_runs_each_build_their_own_instance() {
    let app = app("user-scopes-sequential").await;
    let sync = opener(&app);
    let first = sync
        .builder(CustomerSync { customer: 1 })
        .try_run(|client: Arc<CustomerClient>| async move { ok(client) })
        .await
        .expect("the first run succeeds");
    let second = sync
        .builder(CustomerSync { customer: 2 })
        .try_run(|client: Arc<CustomerClient>| async move { ok(client) })
        .await
        .expect("the second run succeeds");

    assert_eq!(first.customer(), 1);
    assert_eq!(second.customer(), 2);
    assert!(!Arc::ptr_eq(&first, &second));
}
