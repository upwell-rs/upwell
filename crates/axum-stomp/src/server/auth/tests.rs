use std::sync::Arc;

use upwell_axum::WebsocketUpgradeMeta;

use super::{StompConnect, StompPrincipal};

#[test]
fn connect_debug_redacts_the_passcode() {
    let connect = StompConnect::new(
        "example.test".to_owned(),
        Some("alice".to_owned()),
        Some("super-secret".to_owned()),
        vec![("authorization".to_owned(), "Bearer secret-token".to_owned())],
        Arc::new(WebsocketUpgradeMeta::from_parts(
            upwell_axum::axum::http::Method::GET,
            "/stomp?token=hidden".parse().expect("a valid URI"),
            upwell_axum::axum::http::HeaderMap::new(),
        )),
    );
    let debug = format!("{connect:?}");

    assert!(debug.contains("alice"));
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("authorization"));
    assert!(!debug.contains("super-secret"));
    assert!(!debug.contains("secret-token"));
    assert!(debug.contains("/stomp?token=[REDACTED]"), "{debug}");
    assert!(
        !debug.contains("hidden"),
        "the query credential is redacted: {debug}"
    );
}

#[test]
fn principal_carries_subject_and_attributes() {
    let principal = StompPrincipal::authenticated("alice").with_attribute("role", "admin");

    assert!(principal.is_authenticated());
    assert_eq!(principal.subject(), Some("alice"));
    assert_eq!(principal.attribute("role"), Some("admin"));
}
