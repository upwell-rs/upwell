//! Instances a caller opened and let go, as a protocol does for a request or connection.

use upwell::ScopeEndKind;

use super::*;

#[tokio::test]
async fn dropping_the_last_handle_ends_the_instance_once() {
    let app = app("scope-end-handle").await;
    let scope = opener(&app)
        .builder(ended(10))
        .open_default()
        .await
        .expect("the instance opens");
    let clone = scope.clone();

    drop(scope);
    tokio::task::yield_now().await;

    assert!(ends_of(10).is_empty(), "a handle is still open");

    drop(clone);

    assert_eq!(end_of(10).await.kind, ScopeEndKind::Released);
    assert_eq!(ends_of(10).len(), 1);
}

#[tokio::test]
async fn an_instance_that_never_opened_its_container_still_ends() {
    let app = app("scope-end-deferred").await;
    let scope = opener(&app)
        .builder(ended(11))
        .open_when_needed(None)
        .await
        .expect("the instance opens");

    drop(scope);

    assert_eq!(end_of(11).await.kind, ScopeEndKind::Released);
}
