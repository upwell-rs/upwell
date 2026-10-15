//! The native RPC protocol.
//!
//! [`RpcRuntime`] implements the [`ProtocolRuntime`]/[`Serve`] traits from `upwell-app`: it owns the
//! router + middleware stack and drives the per-connection / per-call loop over any
//! [`Transport`](upwell_transport::Transport), opening connection and request scopes
//! through the [`AppRuntime`]. The serve envelope (lifecycle hooks, reload triggers,
//! ctrl-c) is run by `App::serve`, so this loop only watches its transport and the
//! shutdown signal.

use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

use futures::{FutureExt, StreamExt};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tower::{Service, ServiceExt};
use tracing::{debug, error, info, instrument, warn};
use upwell_app::{AppRuntime, ProtocolRuntime, Serve, ShutdownSignal};
use upwell_core::TypeDescriptor;
use upwell_di::{BoxedComponent, ScopeContainer};
use upwell_transport::{
    CallResult, Connection, Error as TransportError, PeerInfo, PredefinedCode, Respond,
    RespondStream, ResponseSink, StatusCode, Transport,
};

use crate::descriptors::{RpcCallContext, RpcOutcome, RpcResponse};
use crate::extract::ErrorResponse;
use crate::middleware::{ErrorHandler, RpcRequest, RpcService};
use crate::router::RpcRouter;
use crate::scope::{Connection as ConnectionScope, Request as RequestScope};

/// Default number of concurrently served RPC connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 1024;

/// Default number of handler tasks allowed on one RPC connection.
pub const DEFAULT_MAX_CALLS_PER_CONNECTION: usize = 256;

const DEFAULT_ACCEPT_RETRY_INITIAL: Duration = Duration::from_millis(25);
const DEFAULT_ACCEPT_RETRY_MAX: Duration = Duration::from_secs(1);

/// Admission and transient-accept limits for the RPC server.
#[derive(Clone, Copy, Debug)]
pub struct RpcLimits {
    max_connections: usize,
    max_calls_per_connection: usize,
    accept_retry_initial: Duration,
    accept_retry_max: Duration,
}

impl RpcLimits {
    pub fn new(max_connections: usize, max_calls_per_connection: usize) -> Self {
        assert!(max_connections > 0, "maximum connections must be non-zero");
        assert!(
            max_calls_per_connection > 0,
            "maximum calls per connection must be non-zero"
        );

        Self {
            max_connections,
            max_calls_per_connection,
            accept_retry_initial: DEFAULT_ACCEPT_RETRY_INITIAL,
            accept_retry_max: DEFAULT_ACCEPT_RETRY_MAX,
        }
    }

    pub fn with_accept_backoff(mut self, initial: Duration, maximum: Duration) -> Self {
        assert!(
            !initial.is_zero(),
            "initial accept backoff must be non-zero"
        );
        assert!(
            maximum >= initial,
            "maximum accept backoff is below initial"
        );
        self.accept_retry_initial = initial;
        self.accept_retry_max = maximum;
        self
    }

    pub fn max_connections(self) -> usize {
        self.max_connections
    }

    pub fn max_calls_per_connection(self) -> usize {
        self.max_calls_per_connection
    }
}

impl Default for RpcLimits {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_CALLS_PER_CONNECTION)
    }
}

/// The built native RPC runtime served over a [`Transport`].
pub struct RpcRuntime {
    router: Arc<RpcRouter>,
    service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    needs_peer: bool,
    limits: RpcLimits,
}

impl RpcRuntime {
    pub(crate) fn new(
        router: Arc<RpcRouter>,
        service: RpcService,
        error_handler: Option<Arc<dyn ErrorHandler>>,
        needs_peer: bool,
        limits: RpcLimits,
    ) -> Self {
        Self {
            router,
            service,
            error_handler,
            needs_peer,
            limits,
        }
    }

    /// The number of routes the dispatcher resolves.
    pub fn route_count(&self) -> usize {
        self.router.route_count()
    }

    pub fn limits(&self) -> RpcLimits {
        self.limits
    }
}

impl ProtocolRuntime for RpcRuntime {
    type Error = crate::Error;
}

impl<T> Serve<T> for RpcRuntime
where
    T: Transport,
    T::Connection: 'static,
{
    async fn serve(
        self,
        runtime: AppRuntime,
        mut shutdown: ShutdownSignal,
        mut transport: T,
    ) -> crate::Result<()> {
        let transport_name = std::any::type_name::<T>();

        info!(target: "upwell::daemon", app = runtime.name(), transport = transport_name, "serve starting");

        let connection_cancel = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut serve_error = None;
        let mut accept_retry = self.limits.accept_retry_initial;

        'serve: loop {
            tokio::select! {
                result = transport.accept(), if connections.len() < self.limits.max_connections => {
                    match result {
                        Ok(conn) => {
                            accept_retry = self.limits.accept_retry_initial;
                            debug!(target: "upwell::daemon", peer = ?conn.peer().addr, "connection accepted, spawning task");

                            let service = self.service.clone();
                            let error_handler = self.error_handler.clone();
                            let needs_peer = self.needs_peer;
                            let runtime = runtime.clone();
                            let cancel = connection_cancel.clone();
                            let max_calls = self.limits.max_calls_per_connection;
                            connections.spawn(async move {
                                serve_connection(conn, service, error_handler, needs_peer, runtime, cancel, max_calls)
                                    .await;
                            });
                        }

                        Err(TransportError::Io(e)) if is_transient_accept_error(&e) => {
                            warn!(target: "upwell::daemon", error = %e, retry_in = ?accept_retry, "transient transport accept failure");

                            tokio::select! {
                                _ = tokio::time::sleep(accept_retry) => {}
                                _ = shutdown.wait() => {
                                    info!(target: "upwell::daemon", "shutdown signal received during accept backoff");
                                    connection_cancel.cancel();
                                    break 'serve;
                                }
                            }

                            accept_retry = accept_retry
                                .saturating_mul(2)
                                .min(self.limits.accept_retry_max);
                        }

                        Err(TransportError::Closed) => break,

                        Err(e) => {
                            error!(target: "upwell::daemon", error = %e, "terminal transport accept failure");
                            connection_cancel.cancel();
                            serve_error = Some(e.into());
                            break;
                        }
                    }
                }

                result = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(e)) = result {
                        warn!(target: "upwell::daemon", error = %e, "connection task failed");
                    }
                }

                _ = shutdown.wait() => {
                    info!(target: "upwell::daemon", "shutdown signal received");
                    connection_cancel.cancel();
                    break;
                }
            }
        }

        while !connections.is_empty() {
            tokio::select! {
                result = connections.join_next() => {
                    if let Some(Err(e)) = result {
                        warn!(target: "upwell::daemon", error = %e, "connection task failed during shutdown");
                    }
                }

                _ = shutdown.wait() => {
                    connection_cancel.cancel();
                    break;
                }
            }
        }

        while let Some(result) = connections.join_next().await {
            if let Err(e) = result {
                warn!(target: "upwell::daemon", error = %e, "connection task failed during shutdown");
            }
        }

        info!(target: "upwell::daemon", transport = transport_name, "serve stopped");

        match serve_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn is_transient_accept_error(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::TimedOut
    ) {
        return true;
    }

    // Resource exhaustion reported by accept(2) / Winsock is generally exposed only as a
    // platform raw code. Keep the tables target-specific: small Windows system-error values
    // overlap unrelated Unix errno values (and vice versa).
    is_transient_accept_os_error(error.raw_os_error())
}

#[cfg(windows)]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(
        raw,
        Some(
            4      // ERROR_TOO_MANY_OPEN_FILES
            | 8    // ERROR_NOT_ENOUGH_MEMORY
            | 14   // ERROR_OUTOFMEMORY
            | 10024 // WSAEMFILE
            | 10055 // WSAENOBUFS
        )
    )
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24 | 105)) // ENOMEM, ENFILE, EMFILE, ENOBUFS
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24 | 55)) // ENOMEM, ENFILE, EMFILE, ENOBUFS
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
))]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24)) // ENOMEM, ENFILE, EMFILE
}

#[cfg(not(any(unix, windows)))]
fn is_transient_accept_os_error(_raw: Option<i32>) -> bool {
    false
}

/// Serves one connection: builds the connection scope (seeding the peer when a
/// component depends on it), then drives each inbound call on its own task so
/// streaming calls run concurrently and the connection keeps reading inbound frames
/// while handlers run.
#[instrument(
    target = "upwell::daemon",
    level = "debug",
    skip_all,
    fields(peer = ?conn.peer().addr),
    name = "connection"
)]
async fn serve_connection<C: Connection>(
    mut conn: C,
    service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
    needs_peer: bool,
    runtime: AppRuntime,
    shutdown: CancellationToken,
    max_calls: usize,
) {
    debug!(target: "upwell::daemon", "connection established");

    // The peer (by value — the framework's connection-scoped injectable) is seeded
    // only when a component depends on it; handlers reach it through the `Peer`
    // extractor regardless, so an otherwise-empty connection scope is skipped. A
    // failed factory closes the connection.
    let peer = conn.peer().clone();
    let seeds = if needs_peer {
        vec![BoxedComponent {
            ty: TypeDescriptor::of::<PeerInfo>("PeerInfo"),
            value: Box::new(peer.clone()),
        }]
    } else {
        Vec::new()
    };

    let connection_scope = match runtime.open_scope_from_root(&ConnectionScope, seeds).await {
        Ok(scope) => scope,

        Err(e) => {
            error!(target: "upwell::daemon", error = %e, "connection scope build failed, closing");
            return;
        }
    };

    let mut tasks: JoinSet<()> = JoinSet::new();

    debug!(target: "upwell::daemon", "connection ready");

    loop {
        tokio::select! {
            recv = conn.recv() => match recv {
                Ok(Some((call, responder))) => {
                    while let Some(result) = tasks.try_join_next() {
                        observe_call_task(result);
                    }

                    if tasks.len() >= max_calls {
                        warn!(
                            target: "upwell::daemon",
                            max_calls,
                            "connection exceeded its in-flight call limit; closing"
                        );
                        call.cancel.cancel();
                        break;
                    }

                    let path = call.path;
                    let service = service.clone();
                    let error_handler = error_handler.clone();
                    let connection_scope = Arc::clone(&connection_scope);
                    let runtime = runtime.clone();
                    let peer = peer.clone();

                    debug!(target: "upwell::daemon", %path, "dispatching call");

                    tasks.spawn(drive_call(
                        path,
                        call.payload,
                        call.requests,
                        call.cancel,
                        peer,
                        connection_scope,
                        runtime,
                        responder,
                        service,
                        error_handler,
                    ));
                }

                Ok(None) => {
                    debug!(target: "upwell::daemon", "connection closed by peer");
                    break;
                }

                Err(e) => {
                    warn!(target: "upwell::daemon", error = %e, "connection error");
                    break;
                }
            },

            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(result) = result {
                    observe_call_task(result);
                }
            }

            _ = shutdown.cancelled() => {
                debug!(target: "upwell::daemon", "connection shutdown requested");
                break;
            }
        }
    }

    // The connection (and its call table) is dropped here, cancelling in-flight
    // calls via their tokens; abort any handler tasks still winding down.
    tasks.shutdown().await;

    debug!(target: "upwell::daemon", "connection ended");
}

fn observe_call_task(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        warn!(target: "upwell::daemon", %error, "call task failed");
    }
}

/// Drives one call to completion on its own task: build its request scope, dispatch,
/// then pump the outcome into the matching responder — a single reply for unary calls,
/// or a stream of items terminated by `finish`/`error` for streaming calls.
#[allow(clippy::too_many_arguments)]
async fn drive_call<R>(
    path: String,
    payload: Vec<u8>,
    requests: Option<mpsc::Receiver<Vec<u8>>>,
    cancel: CancellationToken,
    peer: PeerInfo,
    connection_scope: Arc<ScopeContainer>,
    runtime: AppRuntime,
    responder: R,
    mut service: RpcService,
    error_handler: Option<Arc<dyn ErrorHandler>>,
) where
    R: Respond + RespondStream + Send + 'static,
{
    let request_scope =
        match AssertUnwindSafe(runtime.open_scope(&RequestScope, connection_scope, Vec::new()))
            .catch_unwind()
            .await
        {
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

#[cfg(test)]
mod tests;
