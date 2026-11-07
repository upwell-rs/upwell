//! Reading the active instance of a `#[scope]` type: `S::current()`, `S::is_active()` and the
//! declared `Scoped<S, T>`.

use std::sync::Arc;

use upwell::context::{Scoped, ScopedError};

use super::{Batch, CustomerClient, CustomerSync, app, opener};

#[tokio::test]
async fn a_run_reads_its_own_instance_of_the_scope() {
    let app = app("user-scopes-current").await;

    let seen = opener(&app)
        .builder(CustomerSync { customer: 11 })
        .run(|_client: Arc<CustomerClient>| async move {
            (
                CustomerSync::current().map(|sync| sync.customer).ok(),
                CustomerSync::is_active(),
                Batch::is_active(),
            )
        })
        .await
        .expect("the run succeeds");

    assert_eq!(seen, (Some(11), true, false));
}

#[tokio::test]
async fn a_child_run_reads_its_parent_instance() {
    let app = app("user-scopes-current-child").await;
    let parent = opener(&app)
        .builder(CustomerSync { customer: 5 })
        .open_on_demand()
        .await
        .expect("the customer sync opens");
    let batches = app
        .runtime()
        .opener::<Batch>()
        .expect("discovery declares the batch scope");

    let seen = batches
        .builder(Batch)
        .under(&parent)
        .run(|_client: Arc<CustomerClient>| async move {
            let pinned = Scoped::<CustomerSync, Arc<CustomerSync>>::new()
                .get()
                .await
                .map(|sync| sync.customer)
                .ok();

            (
                CustomerSync::current().map(|sync| sync.customer).ok(),
                pinned,
                Batch::is_active(),
            )
        })
        .await
        .expect("the batch runs");

    assert_eq!(seen, (Some(5), Some(5), true));
}

#[tokio::test]
async fn outside_every_instance_the_scope_is_inactive() {
    assert!(!CustomerSync::is_active());
    assert!(matches!(
        CustomerSync::current(),
        Err(ScopedError::Context(_))
    ));
}
