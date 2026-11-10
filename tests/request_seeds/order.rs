//! Which head a reader sees when middleware rewrites it around the scope's opening.
//!
//! A read the route declares sees the head as it was when the scope opened. A read it does not
//! declare captures the head the first time it happens, so it sees rewrites made after the
//! opening. Before #347 every reader saw the head at the opening.

use upwell::axum::axum::middleware::from_fn;

use super::*;

#[tokio::test]
async fn a_rewrite_before_the_scope_opens_is_seen() {
    let router = router(app().layer(from_fn(rewrite))).await;

    let tenant = declared_tenant(get(&router, "/tenant/declared").await).await;

    assert_eq!(tenant.as_deref(), Some("rewritten"));
}

#[tokio::test]
async fn a_declared_read_sees_the_head_from_when_the_scope_opened() {
    let injected = router(app().layer(from_fn(open_unrelated)).layer(from_fn(rewrite))).await;
    let handled = router(app().layer(from_fn(open_handle)).layer(from_fn(rewrite))).await;

    let injected = declared_tenant(get(&injected, "/tenant/declared").await).await;
    let handled = declared_tenant(get(&handled, "/tenant/declared").await).await;

    assert_eq!(
        injected.as_deref(),
        Some("original"),
        "opened by an injection"
    );
    assert_eq!(handled.as_deref(), Some("original"), "opened by a handle");
}

#[tokio::test]
async fn an_undeclared_read_sees_the_head_from_when_it_first_reads() {
    let router = router(
        app()
            .layer(from_fn(open_unrelated))
            .layer(from_fn(rewrite))
            .layer(from_fn(read_reader)),
    )
    .await;

    let response = get(&router, "/tenant/undeclared").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        seen(&response),
        Some("rewritten"),
        "the route's opening did not read the head, so the first read captured it"
    );
}

#[tokio::test]
async fn a_handle_captures_the_head_when_it_is_handed_out() {
    let router = router(
        app()
            .layer(from_fn(open_handle))
            .layer(from_fn(rewrite))
            .layer(from_fn(read_reader)),
    )
    .await;

    let response = get(&router, "/tenant/undeclared").await;

    assert_eq!(seen(&response), Some("original"));
}

#[tokio::test]
async fn a_request_no_route_matches_builds_the_whole_scope_from_its_head() {
    let router = router(app().layer(from_fn(rewrite)).layer(from_fn(read_reader))).await;

    let response = get(&router, "/nowhere").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(seen(&response), Some("rewritten"));
}
