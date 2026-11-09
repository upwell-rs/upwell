//! What the upgrade metadata shows when it is formatted for logs.

use axum::http::{HeaderMap, HeaderValue, Method};

use super::super::WebsocketUpgradeMeta;

/// Upgrade metadata whose authorization header, cookie, query value and bare query segment
/// each hold a distinct secret.
fn with_secrets() -> WebsocketUpgradeMeta {
    let mut headers = HeaderMap::new();

    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer header-secret"),
    );
    headers.insert("cookie", HeaderValue::from_static("session=cookie-secret"));

    WebsocketUpgradeMeta::from_parts(
        Method::GET,
        "/ws?token=query-secret&bare-secret"
            .parse()
            .expect("a valid URI"),
        headers,
    )
}

#[test]
fn debug_shows_names_and_no_secret() {
    let debug = format!("{:?}", with_secrets());

    for secret in [
        "header-secret",
        "cookie-secret",
        "query-secret",
        "bare-secret",
    ] {
        assert!(!debug.contains(secret), "`{secret}` leaks: {debug}");
    }

    for name in ["authorization", "cookie", "session", "token"] {
        assert!(debug.contains(name), "`{name}` is named: {debug}");
    }
}

#[test]
fn a_bare_query_segment_is_redacted_whole() {
    assert_eq!(
        with_secrets().redacted_uri(),
        "/ws?token=[REDACTED]&[REDACTED]"
    );
}
