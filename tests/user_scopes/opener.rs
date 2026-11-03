//! Opening instances through injected and prepared openers.

use std::sync::Arc;

use std::sync::atomic::Ordering;

use upwell::daemon::App;
use upwell::{AppError, EntryKey, ScopeOpener};

use super::{
    Batch, BatchStep, CustomerClient, CustomerSync, Syncer, UNUSED_BUILDS, Unused, app, opener,
};

#[tokio::test]
async fn an_injected_opener_runs_work_in_a_fresh_instance() {
    let app = app("user-scopes-injected").await;
    let syncer = app
        .container()
        .extract::<Arc<Syncer>>()
        .await
        .expect("the syncer builds with its opener");

    assert_eq!(syncer.sync(7).await.expect("the run succeeds"), 7);
    assert_eq!(syncer.sync(8).await.expect("the run succeeds"), 8);
}

#[tokio::test]
async fn an_injected_opener_does_not_keep_its_runtime_alive() {
    let app = app("user-scopes-weak").await;
    let syncer = app
        .container()
        .extract::<Arc<Syncer>>()
        .await
        .expect("the syncer builds with its opener");

    drop(app);

    let error = syncer
        .sync(7)
        .await
        .expect_err("the runtime is gone once the application drops");

    assert!(matches!(error, AppError::RuntimeUnavailable));
}

#[tokio::test]
async fn a_child_scope_runs_under_a_typed_parent_instance() {
    let app = app("user-scopes-under").await;
    let parent = opener(&app)
        .builder(CustomerSync { customer: 3 })
        .open_on_demand()
        .await
        .expect("the customer sync opens");
    let batches = app
        .runtime()
        .opener::<Batch>()
        .expect("discovery declares the batch scope");

    let customer = batches
        .builder(Batch)
        .under(&parent)
        .run(|client: Arc<CustomerClient>| async move { client.customer() })
        .await
        .expect("the batch runs");

    assert_eq!(customer, 3);
}

#[tokio::test]
async fn a_seed_without_a_slot_fails_the_run() {
    let app = app("user-scopes-seed").await;

    let error = opener(&app)
        .builder(CustomerSync { customer: 1 })
        .seed(5_u32)
        .run(|| async {})
        .await
        .expect_err("no slot takes a `u32` seed");

    assert!(matches!(error, AppError::Di(_)), "{error}");
}

#[tokio::test]
async fn an_opener_extracted_from_the_root_opens_instances() {
    let app = app("user-scopes-extract").await;
    let sync = app
        .container()
        .extract::<ScopeOpener<CustomerSync>>()
        .await
        .expect("every application can inject an opener");

    let customer = sync
        .builder(CustomerSync { customer: 9 })
        .run(|params: Arc<CustomerSync>| async move { params.customer })
        .await
        .expect("the run reads the instance's value");

    assert_eq!(customer, 9);
}

#[tokio::test]
async fn an_injected_opener_rejects_a_scope_the_application_does_not_declare() {
    let app = App::builder("user-scopes-undeclared")
        .build()
        .await
        .expect("the application builds without discovery");
    let sync = app
        .container()
        .extract::<ScopeOpener<CustomerSync>>()
        .await
        .expect("injecting an opener checks nothing until it opens");

    let error = sync
        .builder(CustomerSync { customer: 1 })
        .run(|| async {})
        .await
        .expect_err("the application does not declare the scope");

    assert!(
        matches!(error, AppError::UndeclaredScopeOpen { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_registered_entry_opens_with_its_plan_and_the_value() {
    let app = app("user-scopes-entry").await;
    let sync = opener(&app);
    let entry = sync
        .entry(EntryKey::new("app/reconcile").expect("valid entry key"))
        .expect("discovery registers the entry");

    let instance = sync
        .builder(CustomerSync { customer: 4 })
        .open(&entry)
        .await
        .expect("the instance opens");

    let client = instance
        .container()
        .get::<CustomerClient>()
        .expect("the entry's plan builds its target");

    assert_eq!(client.customer(), 4);
    assert!(instance.container().get::<Unused>().is_none());
}

#[tokio::test]
async fn opening_every_slot_still_builds_what_a_run_leaves_out() {
    let app = app("user-scopes-default").await;

    opener(&app)
        .builder(CustomerSync { customer: 1 })
        .open_default()
        .await
        .expect("the instance opens");

    assert_eq!(UNUSED_BUILDS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_target_without_a_provider_fails_the_run() {
    /// Provided by nothing.
    struct Missing;

    let app = app("user-scopes-missing").await;

    let error = opener(&app)
        .builder(CustomerSync { customer: 1 })
        .run(|_missing: Arc<Missing>| async {})
        .await
        .expect_err("nothing provides the target");

    assert!(matches!(error, AppError::Di(_)), "{error}");
}

#[tokio::test]
async fn a_target_in_a_shorter_lived_scope_fails_the_run() {
    let app = app("user-scopes-unreachable").await;

    let error = opener(&app)
        .builder(CustomerSync { customer: 1 })
        .run(|_step: Arc<BatchStep>| async {})
        .await
        .expect_err("a batch step lives below the customer sync");

    assert!(matches!(error, AppError::Di(_)), "{error}");
}

#[tokio::test]
async fn a_run_declares_its_targets_under_deny_unplanned() {
    let app = App::builder("user-scopes-strict")
        .auto_discover()
        .deny_unplanned()
        .build()
        .await
        .expect("the application builds");

    let customer = opener(&app)
        .builder(CustomerSync { customer: 2 })
        .run(|client: Arc<CustomerClient>| async move { client.customer() })
        .await
        .expect("the run's parameters declare what it builds");

    assert_eq!(customer, 2);
}

#[tokio::test]
async fn a_run_survives_a_reload_of_the_runtime() {
    let app = app("user-scopes-reload").await;
    let sync = opener(&app);

    app.runtime()
        .reload_config()
        .await
        .expect("an unchanged reload commits");

    let customer = sync
        .builder(CustomerSync { customer: 6 })
        .run(|client: Arc<CustomerClient>| async move { client.customer() })
        .await
        .expect("the run opens on the new generation");

    assert_eq!(customer, 6);
}
