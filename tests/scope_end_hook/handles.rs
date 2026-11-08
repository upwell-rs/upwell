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

#[tokio::test]
async fn a_deferred_entry_scope_that_never_opens_ends_when_dropped() {
    let app = app("scope-end-deferred-entry").await;
    let opener = opener(&app);
    let entry = opener
        .entry(upwell::EntryKey::new("scope-end/nothing").expect("valid entry key"))
        .expect("the entry exists");
    let scope = opener
        .builder(ended(12))
        .open_when_needed(Some(&entry))
        .await
        .expect("the instance opens");

    assert!(!scope.is_open(), "the entry plans nothing, so it defers");

    drop(scope);

    assert_eq!(end_of(12).await.kind, ScopeEndKind::Released);
}

#[tokio::test]
async fn an_instance_that_fails_to_open_ends_nothing() {
    let app = app("scope-end-failed-open").await;
    let failing = app
        .runtime()
        .opener::<Failing>()
        .expect("discovery declares the failing scope");

    failing
        .builder(Failing { tag: 13 })
        .open_default()
        .await
        .err()
        .expect("the broken component fails the opening");
    opener(&app)
        .builder(ended(14))
        .run(|| async {})
        .await
        .expect("the control run succeeds");

    end_of(14).await;

    assert!(ends_of(13).is_empty());
}

#[tokio::test]
async fn the_container_keeps_the_instance_alive_after_its_handle_goes() {
    let app = app("scope-end-into-container").await;
    let container = opener(&app)
        .builder(ended(15))
        .open_default()
        .await
        .expect("the instance opens")
        .into_container();

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(ends_of(15).is_empty(), "the container is still in use");

    drop(container);

    assert_eq!(end_of(15).await.kind, ScopeEndKind::Released);
}
