//! Tests for the STOMP serve-loop helpers (see the parent [`super`] module).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use stomp_parser::client::ClientFrame;

use upwell_axum::{AxumAppBuilder, WebsocketProtocol};

use crate::Stomp;
use crate::server::{
    IntoStompOutcome, STOMP_HEADERS_DESCRIPTOR, STOMP_PRINCIPAL_DESCRIPTOR,
    STOMP_SESSION_DESCRIPTOR, ensure_connect_host, send_header_seed,
};

struct SensitiveError {
    secret: String,
    dropped: Arc<AtomicBool>,
}

impl Drop for SensitiveError {
    fn drop(&mut self) {
        let _ = &self.secret;
        self.dropped.store(true, Ordering::Relaxed);
    }
}

#[test]
fn host_is_injected_so_a_hostless_connect_parses() {
    // A stomp.js-style CONNECT with no `host` header — rejected by stomp-parser as-is.
    let frame = b"CONNECT\naccept-version:1.0,1.1,1.2\nheart-beat:0,0\n\n\x00".to_vec();
    assert!(
        ClientFrame::try_from(frame.clone()).is_err(),
        "hostless CONNECT is rejected raw"
    );

    let patched = ensure_connect_host(frame);
    let parsed = ClientFrame::try_from(patched).expect("patched CONNECT parses");

    assert!(matches!(parsed, ClientFrame::Connect(_)));
}

#[test]
fn a_connect_with_a_host_is_left_untouched() {
    let frame = b"CONNECT\naccept-version:1.2\nhost:example\n\n\x00".to_vec();
    let out = ensure_connect_host(frame.clone());

    assert_eq!(out, frame, "an existing host is not duplicated");
}

#[test]
fn non_connect_frames_are_left_untouched() {
    let frame = b"SEND\ndestination:/app/chat\n\nhi\x00".to_vec();
    let out = ensure_connect_host(frame.clone());

    assert_eq!(out, frame);
}

#[test]
fn generic_result_discards_application_error_without_formatting_or_retaining_it() {
    // Deliberately does not implement Display: accepting it proves the generic Result path cannot
    // stringify application errors. The source value is dropped and only a fixed category remains.
    let dropped = Arc::new(AtomicBool::new(false));
    let result: Result<(), SensitiveError> = Err(SensitiveError {
        secret: "postgres://user:secret@host/db".to_owned(),
        dropped: Arc::clone(&dropped),
    });

    let error = match result.into_outcome() {
        Ok(_) => panic!("application failure unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(dropped.load(Ordering::Relaxed));
    assert!(matches!(error, upwell_axum::WsDispatchError::Application));
    assert_eq!(error.to_string(), "ws application error");
    assert!(!error.to_string().contains("secret"));
}

/// Regression test: a `SEND`'s custom headers (e.g. correlation or auth metadata) must reach
/// the handler's `Inject<StompHeaders>`, not just `destination`/`content-type`.
#[test]
fn send_header_seed_carries_custom_headers_through() {
    let headers = send_header_seed(
        "/app/chat",
        Some("application/json".to_owned()),
        vec![("correlation-id".to_owned(), "abc-123".to_owned())],
    );

    assert_eq!(
        headers,
        vec![
            ("destination".to_owned(), "/app/chat".to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("correlation-id".to_owned(), "abc-123".to_owned()),
        ]
    );
}

#[test]
fn stomp_registers_every_message_seed_at_the_message_destination() {
    let mut registry = upwell_axum::AppRegistry::default();

    <Stomp as WebsocketProtocol>::register(&mut registry);

    for descriptor in [
        STOMP_HEADERS_DESCRIPTOR,
        STOMP_SESSION_DESCRIPTOR,
        STOMP_PRINCIPAL_DESCRIPTOR,
    ] {
        assert_eq!(
            descriptor.scope.id(),
            <upwell_axum::WebsocketMessage as upwell_axum::Scope>::ID
        );
        assert!(
            descriptor
                .effective_factory()
                .expect("seed descriptor is unambiguous")
                .is_none()
        );
        assert!(
            registry
                .components
                .iter()
                .any(|registered| registered.ty.type_id == descriptor.ty.type_id)
        );
    }
}

#[tokio::test]
async fn stomp_seed_descriptors_allow_app_validation() {
    let app = upwell_axum::App::builder("stomp-seed-validation")
        .register_ws::<Stomp>("/stomp")
        .build()
        .await;

    if let Err(error) = app {
        panic!("STOMP app failed to build: {error}");
    }
}
