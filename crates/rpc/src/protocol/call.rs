//! Driving one call: its request scope, middleware, handler and reply.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::{Service, ServiceExt};
use tracing::{debug, error, warn};
use upwell_app::OpenScope;
use upwell_di::context::bind_opened_scope;
use upwell_transport::{
    CallResult, PeerInfo, PredefinedCode, Respond, RespondStream, ResponseSink, StatusCode,
};

use super::RpcScopes;
use crate::descriptors::{RpcCallContext, RpcOutcome, RpcResponse};
use crate::extract::ErrorResponse;
use crate::middleware::{ErrorHandler, RpcRequest, RpcService};
use crate::scope::{Connection as ConnectionScope, Request as RequestScope};

/// Drives one call to completion on its own task: build its request scope, dispatch,
/// then pump the outcome into the matching responder — a single reply for unary calls,
/// or a stream of items terminated by `finish`/`error` for streaming calls.
///
/// Runs in the call's invocation context, which waits for the call's scope: the scope binds when
/// it opens, here for an entry that builds something and on its first resolution otherwise.
/// Until then, the scope's factories and the global error handler see the connection scope.
#[allow(clippy::too_many_arguments)]
pub(super) async fn drive_call<R>(
    path: String,
    payload: Vec<u8>,
    requests: Option<mpsc::Receiver<Vec<u8>>>,
    cancel: CancellationToken,
    peer: PeerInfo,
    connection_scope: OpenScope<ConnectionScope>,
    scopes: Arc<RpcScopes>,
    responder: R,
    mut service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
) where
    R: Respond + RespondStream + Send + 'static,
{
    let builder = scopes.calls.builder(RequestScope).under(&connection_scope);
    let opening = builder.open_when_needed(scopes.entries.get(path.as_str()));
    let request_scope = match AssertUnwindSafe(opening).catch_unwind().await {
        Ok(Ok(scope)) => scope,

        Ok(Err(e)) => {
            error!(target: "upwell::daemon", %path, error = %e, "request scope build failed");
            let response = apply_error_handler(
                &error_handler,
                &path,
                ErrorResponse::from(crate::Error::from(e)),
            )
            .await;
            let _ = responder
                .respond(CallResult::Err {
                    code: response.code,
                    body: response.body,
                })
                .await;

            return;
        }

        Err(_) => {
            error!(target: "upwell::daemon", %path, "request scope build panicked");
            let response =
                apply_error_handler(&error_handler, &path, internal_error_response()).await;
            let _ = responder
                .respond(CallResult::Err {
                    code: response.code,
                    body: response.body,
                })
                .await;

            return;
        }
    };

    if request_scope.is_open() {
        bind_opened_scope(request_scope.current());
    }

    let ctx = RpcCallContext::new(payload, peer, request_scope, requests, cancel);
    let request = RpcRequest::new(path.clone(), ctx);

    // Drive the request through the middleware stack; its terminal service is the
    // router. `ready` honours the tower contract for layers that exert backpressure.
    let outcome = match AssertUnwindSafe(async {
        match service.ready().await {
            Ok(svc) => svc.call(request).await,

            Err(e) => Err(e),
        }
    })
    .catch_unwind()
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            error!(target: "upwell::daemon", %path, "call handler panicked");
            let response =
                apply_error_handler(&error_handler, &path, internal_error_response()).await;

            if let Err(error) = responder
                .respond(CallResult::Err {
                    code: response.code,
                    body: response.body,
                })
                .await
            {
                warn!(target: "upwell::daemon", %path, %error, "failed to send panic response");
            }

            return;
        }
    };

    match outcome {
        Ok(RpcOutcome::Unary(RpcResponse { payload })) => {
            debug!(target: "upwell::daemon", %path, "call succeeded");

            if let Err(e) = responder.respond(CallResult::Ok(payload)).await {
                warn!(target: "upwell::daemon", %path, error = %e, "failed to send response");
            }
        }

        Ok(RpcOutcome::Stream(mut stream)) => {
            debug!(target: "upwell::daemon", %path, "streaming response");

            let mut sink = responder.into_sink();

            loop {
                match AssertUnwindSafe(stream.next()).catch_unwind().await {
                    Ok(Some(Ok(item))) => {
                        if let Err(e) = sink.send(item).await {
                            warn!(target: "upwell::daemon", %path, error = %e, "failed to send stream item");

                            return;
                        }
                    }

                    Ok(Some(Err(e))) => {
                        warn!(target: "upwell::daemon", %path, code = ?e.code, "stream handler errored");
                        let e = apply_error_handler(&error_handler, &path, e).await;
                        let _ = sink.error(e.code, e.body).await;

                        return;
                    }

                    Ok(None) => break,

                    Err(_) => {
                        error!(target: "upwell::daemon", %path, "response stream panicked");
                        let response =
                            apply_error_handler(&error_handler, &path, internal_error_response())
                                .await;
                        let _ = sink.error(response.code, response.body).await;

                        return;
                    }
                }
            }

            if let Err(e) = sink.finish().await {
                warn!(target: "upwell::daemon", %path, error = %e, "failed to finish stream");
            }
        }

        Err(e) => {
            warn!(target: "upwell::daemon", %path, code = ?e.code, "call returned error");
            let e = apply_error_handler(&error_handler, &path, e).await;

            if let Err(e) = responder
                .respond(CallResult::Err {
                    code: e.code,
                    body: e.body,
                })
                .await
            {
                warn!(target: "upwell::daemon", %path, error = %e, "failed to send error response");
            }
        }
    }
}

fn internal_error_response() -> ErrorResponse {
    ErrorResponse::with_serialized_body(
        StatusCode::from(PredefinedCode::Internal),
        "internal server error",
    )
}

/// Applies the global [`ErrorHandler`] to an outgoing error, or passes it through
/// unchanged when none is registered.
async fn apply_error_handler(
    handler: &Option<Arc<dyn ErrorHandler>>,
    path: &str,
    error: ErrorResponse,
) -> ErrorResponse {
    match handler {
        Some(handler) => match AssertUnwindSafe(async { handler.handle(path, error).await })
            .catch_unwind()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                error!(target: "upwell::daemon", %path, "global error handler panicked");

                internal_error_response()
            }
        },

        None => error,
    }
}
