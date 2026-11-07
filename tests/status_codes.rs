//! End-to-end tests for the error-response status code, driven over the
//! in-memory transport so they are fast and deterministic.
#![cfg(feature = "daemon")]

mod common;

use upwell::daemon::{App, ErrorResponse, ResponseError, ResponseStream, handlers, service};
use upwell::{CallResult, Flags, PredefinedCode, ServerEvent, StatusCode};

use common::{MemoryServer, deadline};

// ---------------------------------------------------------------------------
// A service whose handlers return classified errors.
// ---------------------------------------------------------------------------

/// Application subcode carried in the custom section of an `AppError`.
const SUBCODE: u16 = 0x0042;

/// A structured error body, to prove the body round-trips intact.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct AppErrorBody {
    detail: String,
    retry_after_ms: u32,
}

/// A handler error carrying a predefined category, a custom subcode, the
/// `RETRYABLE` flag, and a structured body.
#[derive(Debug)]
struct AppError;

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "application boom")
    }
}

impl ResponseError for AppError {
    type Body = AppErrorBody;

    fn error_response(self) -> ErrorResponse {
        let body = AppErrorBody {
            detail: "bad thing".to_string(),
            retry_after_ms: 100,
        };

        ErrorResponse::with_serialized_body(self.status_code(), &body)
    }

    fn status_code(&self) -> StatusCode {
        StatusCode::new_with_custom(PredefinedCode::BadInput, Flags::RETRYABLE, SUBCODE)
    }
}

/// Test service exercising the classified-error path.
#[service(id = "status_svc", version = "0.1")]
struct StatusSvc;

#[handlers]
impl StatusSvc {
    /// A handler returning a custom error type with code + structured body.
    #[rpc]
    async fn custom_error() -> Result<u32, AppError> {
        Err(AppError)
    }

    /// An unchanged framework-error handler, mapped to its category.
    #[rpc]
    async fn framework_error() -> upwell::daemon::Result<u32> {
        Err(upwell::daemon::Error::InvalidPayload("nope".to_string()))
    }

    /// A server stream that yields items then fails, terminating with the same
    /// `{ code, body }` shape as a unary error.
    #[rpc]
    async fn stream_then_fail() -> ResponseStream<u32> {
        ResponseStream::new(futures::stream::iter(vec![
            Ok(0u32),
            Ok(1u32),
            Err(upwell::daemon::Error::InvalidPayload(
                "mid-stream".to_string(),
            )),
        ]))
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn start() -> MemoryServer {
    let daemon = App::builder("test")
        .auto_discover()
        .build()
        .await
        .expect("build daemon");

    MemoryServer::start(daemon)
}

fn enc<T: serde::Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).unwrap()
}

fn dec<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    postcard::from_bytes(bytes).unwrap()
}

// ---------------------------------------------------------------------------
// US1: typed status code and structured body on error responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn custom_error_carries_code_and_body() {
    // SC-001: the client reads the exact predefined code and deserializes the
    // exact structured body.
    let server = start().await;
    let conn = server.connect().await;

    let result = deadline(
        "StatusSvc.custom_error call",
        conn.call("StatusSvc.custom_error", enc(&())),
    )
    .await
    .expect("custom error call returns a response");

    match result {
        CallResult::Err { code, body } => {
            assert_eq!(code.predefined(), PredefinedCode::BadInput);

            let decoded: AppErrorBody = dec(&body);

            assert_eq!(
                decoded,
                AppErrorBody {
                    detail: "bad thing".to_string(),
                    retry_after_ms: 100,
                }
            );
        }

        other => panic!("expected an error response, got {other:?}"),
    }

    server.shutdown([conn]).await;
}

#[tokio::test]
async fn framework_error_handler_maps_to_category() {
    // SC-003: an unchanged `Result<T, upwell::daemon::Error>` handler still works and
    // maps to its predefined category (InvalidPayload -> BadInput).
    let server = start().await;
    let conn = server.connect().await;

    let result = deadline(
        "StatusSvc.framework_error call",
        conn.call("StatusSvc.framework_error", enc(&())),
    )
    .await
    .expect("framework error call returns a response");

    match result {
        CallResult::Err { code, .. } => {
            assert_eq!(code.predefined(), PredefinedCode::BadInput);
        }

        other => panic!("expected an error response, got {other:?}"),
    }

    server.shutdown([conn]).await;
}

// ---------------------------------------------------------------------------
// US2: custom application codes that cannot collide with framework codes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn custom_subcode_round_trips_with_predefined() {
    // SC-005: both the predefined category and the custom subcode survive the
    // round-trip intact, in their own sections.
    let server = start().await;
    let conn = server.connect().await;

    let result = deadline(
        "StatusSvc.custom_error call",
        conn.call("StatusSvc.custom_error", enc(&())),
    )
    .await
    .expect("custom error call returns a response");

    match result {
        CallResult::Err { code, .. } => {
            assert_eq!(code.predefined(), PredefinedCode::BadInput);
            assert_eq!(code.custom(), SUBCODE);
        }

        other => panic!("expected an error response, got {other:?}"),
    }

    server.shutdown([conn]).await;
}

// ---------------------------------------------------------------------------
// US3: control-flow flags carried alongside the code
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retryable_flag_round_trips() {
    // SC-004: the client detects RETRYABLE from the code alone, without
    // deserializing the body.
    let server = start().await;
    let conn = server.connect().await;

    let result = deadline(
        "StatusSvc.custom_error call",
        conn.call("StatusSvc.custom_error", enc(&())),
    )
    .await
    .expect("custom error call returns a response");

    match result {
        CallResult::Err { code, .. } => {
            assert!(code.contains(Flags::RETRYABLE));
        }

        other => panic!("expected an error response, got {other:?}"),
    }

    server.shutdown([conn]).await;
}

// ---------------------------------------------------------------------------
// FR-010 / SC-006: streaming errors carry the same { code, body } shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_error_has_same_shape_as_unary() {
    let server = start().await;
    let conn = server.connect().await;

    let mut call = deadline(
        "StatusSvc.stream_then_fail open",
        conn.open("StatusSvc.stream_then_fail", enc(&()), false),
    )
    .await
    .expect("open failing stream");

    let mut items = Vec::new();

    loop {
        match deadline("StatusSvc.stream_then_fail receive", call.recv()).await {
            Some(ServerEvent::Item(bytes)) => items.push(dec::<u32>(&bytes)),

            Some(ServerEvent::Error { code, .. }) => {
                // Items before the error are delivered, then the stream
                // terminates with a classified error (mapped from the framework
                // error, BadInput).
                assert_eq!(items, vec![0, 1]);
                assert_eq!(code.predefined(), PredefinedCode::BadInput);

                break;
            }

            other => panic!("expected items then an error, got {other:?}"),
        }
    }

    server.shutdown([conn]).await;
}
