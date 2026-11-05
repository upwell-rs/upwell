#![cfg(feature = "daemon")]

//! End-to-end tests for the invocation context of RPC calls, over the in-memory transport.
//!
//! Each call runs in its own context, middleware, handler, response stream and global error
//! handler included. The call's scope joins it when it opens, so until then the connection
//! scope is the innermost one.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use common::{MemoryServer, deadline};
use upwell::daemon::tower::{Layer, Service};
use upwell::daemon::{
    App, Connection, ErrorHandler, ErrorResponse, Inject, Payload, Request, ResponseStream,
    RpcAppBuilder, RpcOutcome, RpcRequest, RpcTarget, handlers, service,
};
use upwell::{CallResult, MemoryConnectionHandle, Scope, ServerEvent, component, methods};
use upwell_di::context::InvocationContext;

/// What the error handler saw last.
static HANDLER_SAW: Mutex<Option<String>> = Mutex::new(None);

/// What middleware saw of each call, after the handler returned.
static MIDDLEWARE_SAW: Mutex<Vec<InvocationContext>> = Mutex::new(Vec::new());

/// What handlers saw of each call.
static HANDLERS_SAW: Mutex<Vec<InvocationContext>> = Mutex::new(Vec::new());

/// A component of the call's scope, so a handler injecting it opens the scope at once.
#[component(scope = Request)]
struct CallTicket {}

#[methods]
impl CallTicket {
    #[init]
    async fn init() -> Self {
        Self {}
    }
}

/// `rpc` when the active context is an RPC call's, then its innermost scope.
fn describe() -> String {
    let context = InvocationContext::capture().expect("a call runs in a context");
    let kind = match context.target().downcast_ref::<RpcTarget>() {
        Some(RpcTarget) => "rpc",
        _ => "other",
    };

    let scope = match context.scope_key().map(|key| key.id()) {
        Some(id) if id == Request::ID => "call",
        Some(id) if id == Connection::ID => "connection",
        _ => "other",
    };

    format!("{kind}:{scope}")
}

/// Handlers reporting the context they run in.
#[service(id = "context_svc", version = "0.1")]
struct ContextSvc;

#[handlers]
impl ContextSvc {
    /// Injects nothing, so its call scope never opens.
    #[rpc]
    async fn bare() -> String {
        describe()
    }

    /// Injects a call-scoped component, so its call scope opens before the handler.
    #[rpc]
    async fn scoped(Inject(_ticket): Inject<Arc<CallTicket>>) -> String {
        tokio::task::yield_now().await;

        let context = InvocationContext::capture().expect("a call runs in a context");

        HANDLERS_SAW
            .lock()
            .expect("the record is healthy")
            .push(context);

        describe()
    }

    /// Reports the context from inside the stream, polled after the handler returned.
    #[rpc]
    async fn streamed(Inject(_ticket): Inject<Arc<CallTicket>>) -> ResponseStream<String> {
        ResponseStream::new(futures::stream::unfold(0u32, |sent| async move {
            (sent < 2).then(|| (Ok(describe()), sent + 1))
        }))
    }

    /// Always fails, so the global error handler runs.
    #[rpc]
    async fn failing(Payload(_): Payload<()>) -> upwell::daemon::Result<u32> {
        Err(upwell::daemon::Error::InvalidPayload("nope".to_owned()))
    }
}

/// Records the context the global error handler runs in.
struct RecordingErrorHandler;

impl ErrorHandler for RecordingErrorHandler {
    fn handle<'a>(
        &'a self,
        _path: &'a str,
        error: ErrorResponse,
    ) -> Pin<Box<dyn Future<Output = ErrorResponse> + Send + 'a>> {
        Box::pin(async move {
            *HANDLER_SAW.lock().expect("the record is healthy") = Some(describe());

            error
        })
    }
}

/// Records the context middleware sees once the handler returned.
#[derive(Clone)]
struct RecordLayer;

/// The service [`RecordLayer`] wraps.
#[derive(Clone)]
struct RecordService<S> {
    inner: S,
}

impl<S> Layer<S> for RecordLayer {
    type Service = RecordService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RecordService { inner }
    }
}

impl<S> Service<RpcRequest> for RecordService<S>
where
    S: Service<RpcRequest, Response = RpcOutcome, Error = ErrorResponse> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = RpcOutcome;
    type Error = ErrorResponse;
    type Future = Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RpcRequest) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let outcome = inner.call(request).await;
            let context = InvocationContext::capture().expect("middleware runs in a context");

            MIDDLEWARE_SAW
                .lock()
                .expect("the record is healthy")
                .push(context);

            outcome
        })
    }
}

async fn start() -> MemoryServer {
    let app = App::builder("context-rpc-test")
        .auto_discover()
        .middleware(RecordLayer)
        .error_handler(RecordingErrorHandler)
        .build()
        .await
        .expect("the app builds");

    MemoryServer::start(app)
}

fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("the request encodes")
}

async fn call_string(connection: &MemoryConnectionHandle, path: &str) -> String {
    let result = deadline("the call", connection.call(path, encode(&())))
        .await
        .expect("the transport responds");

    match result {
        CallResult::Ok(body) => postcard::from_bytes(&body).expect("a string reply"),
        CallResult::Err { code, .. } => panic!("{path} failed with {code:?}"),
    }
}

#[tokio::test]
async fn a_handler_sees_the_connection_scope_until_its_call_scope_opens() {
    let server = start().await;
    let connection = server.connect().await;

    assert_eq!(
        call_string(&connection, "ContextSvc.bare").await,
        "rpc:connection"
    );
    assert_eq!(
        call_string(&connection, "ContextSvc.scoped").await,
        "rpc:call"
    );

    server.shutdown([connection]).await;
}

#[tokio::test]
async fn a_response_stream_is_polled_in_the_call_context() {
    let server = start().await;
    let connection = server.connect().await;
    let mut call = deadline(
        "the streaming call",
        connection.open("ContextSvc.streamed", encode(&()), false),
    )
    .await
    .expect("the stream opens");
    let mut items = Vec::new();

    while let Some(ServerEvent::Item(bytes)) = deadline("the next event", call.recv()).await {
        items.push(postcard::from_bytes::<String>(&bytes).expect("a string item"));
    }

    assert_eq!(items, ["rpc:call", "rpc:call"]);

    drop(call);
    server.shutdown([connection]).await;
}

#[tokio::test]
async fn the_global_error_handler_runs_in_the_call_context() {
    let server = start().await;
    let connection = server.connect().await;

    let result = deadline(
        "the failing call",
        connection.call("ContextSvc.failing", encode(&())),
    )
    .await
    .expect("the transport responds");

    assert!(matches!(result, CallResult::Err { .. }));
    assert_eq!(
        HANDLER_SAW
            .lock()
            .expect("the record is healthy")
            .as_deref(),
        Some("rpc:connection")
    );

    server.shutdown([connection]).await;
}

#[tokio::test]
async fn concurrent_calls_share_a_scope_with_their_middleware_and_with_nothing_else() {
    const CALLS: usize = 16;

    let server = start().await;
    let connection = Arc::new(server.connect().await);
    let mut calls = tokio::task::JoinSet::new();

    for _ in 0..CALLS {
        let connection = Arc::clone(&connection);

        calls.spawn(async move { call_string(&connection, "ContextSvc.scoped").await });
    }

    while let Some(reply) = calls.join_next().await {
        assert_eq!(reply.expect("the call task completes"), "rpc:call");
    }

    let handlers = std::mem::take(&mut *HANDLERS_SAW.lock().expect("the record is healthy"));
    let middleware = std::mem::take(&mut *MIDDLEWARE_SAW.lock().expect("the record is healthy"));

    assert_eq!(handlers.len(), CALLS);
    assert_eq!(middleware.len(), CALLS);

    for (index, handler) in handlers.iter().enumerate() {
        let sharing = middleware
            .iter()
            .filter(|seen| seen.same_scope(handler))
            .count();
        let others = handlers[index + 1..]
            .iter()
            .filter(|other| other.same_scope(handler))
            .count();

        assert_eq!(
            sharing, 1,
            "exactly the call's own middleware shares its scope"
        );
        assert_eq!(others, 0, "no other call shares its scope");
    }
}
