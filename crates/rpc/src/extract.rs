//! Typed handlers, parameter extraction, and response conversion, in the style
//! of axum/actix.
//!
//! Instead of receiving the raw `RpcCallContext`, a handler declares its
//! dependencies as parameters (`Payload<T>`, `Streaming<T>`, `Conn`, ...) and
//! returns any type that implements [`Responder`]. Any async fn whose
//! parameters are all `FromContext` and whose return type is a `Responder` then
//! satisfies `Handler`, and `dispatch_with` adapts it to the erased future the
//! router invokes.
//!
//! [`Responder`] decides what a *successful* return value becomes on the wire.
//! A blanket impl serializes any `Serialize` value as a unary body (the default
//! format), so handlers can return `T`, `()`, or `Option<T>` directly, and
//! [`ResponseStream<T>`] (a local type, so no overlap with the blanket) produces
//! a stream of items.
//!
//! Fallibility is handled one layer up, at dispatch. `Result<T, E>` is
//! deliberately *not* a `Responder`: Rust's intercrate coherence refuses to let
//! a blanket `impl<T: Serialize>` coexist with an `impl … for Result` (it will
//! not prove `Result: !Serialize` even though serde never implements it). So a
//! handler returning `Result<R, E>` satisfies [`FallibleHandler`] instead of
//! [`Handler`] — that is where `E: ResponseError` is enforced and `Err` is
//! mapped to a transport error. The `#[rpc]` macro picks the matching
//! `dispatch_*` from the return type, so for a given handler exactly one of the
//! two traits ever applies.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use upwell_transport::{
    PeerInfo, PredefinedCode, StatusCode, StreamDecode, StreamEncode, StreamEncodeError,
};

use upwell_di::FromContainer;

use crate::{
    Error,
    descriptors::{RpcCallContext, RpcOutcome, RpcResponse},
};

/// A value a handler can extract from the call context.
///
/// Extractors run in parameter order before the handler body. Most only read
/// from the context; `Streaming<T>` takes the inbound request stream, so it may
/// appear at most once.
pub trait FromContext: Sized {
    fn from_context(ctx: &RpcCallContext) -> impl Future<Output = crate::Result<Self>> + Send;
}

/// Deserializes the request body into `T`.
pub struct Payload<T>(pub T);

impl<T> FromContext for Payload<T>
where
    T: DeserializeOwned + Send,
{
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        let value =
            postcard::from_bytes(&ctx.payload).map_err(|e| Error::InvalidPayload(e.to_string()))?;

        Ok(Payload(value))
    }
}

/// The remote peer for this call.
///
/// Reads the peer directly off the call context (not the scope chain), so it works
/// whether or not a connection-scoped container exists. A connection-scoped
/// *component* may instead depend on `PeerInfo` by field injection.
pub struct Peer(pub PeerInfo);

impl FromContext for Peer {
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        Ok(Peer(ctx.peer().clone()))
    }
}

/// Injects anything a constructor parameter can be — any [`FromContainer`] — from the call's
/// scope: a component handle (`Arc<T>`, `Dep<T>`, a by-value injectable), every provider of a
/// trait (`Vec<Arc<dyn T>>` / `HashMap<String, Arc<dyn T>>`), an optional component
/// (`Option<Arc<T>>`), or a resolver-backed value such as `Cfg<T>`. Resolution walks the
/// request → connection → singleton chain and constructs a fresh instance when the target is
/// a `Transient`.
///
/// This is how a handler reaches connection- and request-scoped components; the stateful
/// service singleton itself still arrives through `&self`.
pub struct Inject<H>(pub H);

impl<H> FromContext for Inject<H>
where
    H: FromContainer,
{
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        let scope = ctx.scope().container().await.map_err(Error::from)?;

        scope.extract::<H>().await.map(Inject).map_err(Error::from)
    }
}

/// The call's cancellation token, fired when the peer cancels the call or the
/// connection drops. Long-running and streaming handlers observe it to unwind.
pub struct Cancel(pub CancellationToken);

impl FromContext for Cancel {
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        Ok(Cancel(ctx.cancel.clone()))
    }
}

/// The inbound request stream for client- and bidirectional-streaming calls,
/// yielding each item deserialized into `T`. Implements [`Stream`], so handlers
/// drive it with `.next().await` (or any `StreamExt` combinator).
pub struct Streaming<T> {
    inner: Pin<Box<dyn Stream<Item = crate::Result<T>> + Send>>,
}

impl<T> FromContext for Streaming<T>
where
    T: StreamDecode + Send + 'static,
{
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        let rx = ctx.take_requests().ok_or(Error::NotStreaming)?;

        let stream = T::from_frames(ReceiverStream::new(rx))
            .map(|item| item.map_err(|e| Error::InvalidPayload(e.to_string())));

        Ok(Streaming {
            inner: Box::pin(stream),
        })
    }
}

impl<T> Stream for Streaming<T> {
    type Item = crate::Result<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

/// The infallible-item counterpart to [`Streaming<T>`]: yields each inbound item
/// parsed into `T`, ending the stream (with a logged warning) the first time an
/// item fails to decode rather than surfacing a `Result` to the handler.
///
/// This is the type the `#[rpc]` macro feeds a handler parameter declared as
/// `impl Stream<Item = T>` (or a generic `S: Stream<Item = T>`); a parameter
/// declared with fallible items maps to [`Streaming<T>`] instead.
pub struct RequestStream<T> {
    inner: Pin<Box<dyn Stream<Item = T> + Send>>,
}

impl<T> FromContext for RequestStream<T>
where
    T: StreamDecode + Send + 'static,
{
    async fn from_context(ctx: &RpcCallContext) -> crate::Result<Self> {
        let rx = ctx.take_requests().ok_or(Error::NotStreaming)?;

        let stream = T::from_frames(ReceiverStream::new(rx))
            .take_while(|item| {
                if let Err(e) = item {
                    warn!(error = %e, "request stream item failed to decode; ending stream");
                }

                std::future::ready(item.is_ok())
            })
            .filter_map(|item| std::future::ready(item.ok()));

        Ok(RequestStream {
            inner: Box::pin(stream),
        })
    }
}

impl<T> Stream for RequestStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

/// The handler-side error currency: a status code plus a serialized body, carried
/// to the wire error arm of a call (unary or streaming).
#[derive(Debug)]
pub struct ErrorResponse {
    pub code: StatusCode,
    pub body: Vec<u8>,
}

impl ErrorResponse {
    /// Builds an error response from a status code and an already-serialized body.
    pub fn new(code: StatusCode, body: Vec<u8>) -> Self {
        Self { code, body }
    }

    /// Builds an error response by serializing `body`, degrading to an empty body
    /// with a logged warning rather than failing (FR-011): the status code must
    /// survive even when the body cannot be encoded.
    pub fn with_serialized_body<T: Serialize + ?Sized>(code: StatusCode, body: &T) -> Self {
        match postcard::to_allocvec(body) {
            Ok(bytes) => Self::new(code, bytes),

            Err(e) => {
                warn!(error = %e, "failed to serialize error body; falling back to empty body");

                Self::new(code, Vec::new())
            }
        }
    }
}

/// Converts an error into the status code + body sent back to the caller.
///
/// `status_code` defaults to `Internal`, so most implementors override only that.
/// A blanket impl covers any `E: Serialize`, serializing the error value itself
/// as the body under the default code — so a serializable error type is usable as
/// a handler error with no boilerplate. A type wanting a custom code, or a body
/// distinct from its own serialization, implements the trait directly (and is
/// then not `Serialize`, so it does not overlap the blanket). [`Error`] is one
/// such type: it maps its variants to categories via [`Error::status_code`](crate::Error::status_code).
pub trait ResponseError {
    type Body: Serialize;

    /// Renders the error to a code + serialized body, attaching
    /// [`status_code`](Self::status_code).
    fn error_response(self) -> ErrorResponse;

    /// The status code for this error. Defaults to `Internal`.
    fn status_code(&self) -> StatusCode {
        StatusCode::from(PredefinedCode::Internal)
    }
}

impl<E> ResponseError for E
where
    E: Serialize,
{
    type Body = E;

    fn error_response(self) -> ErrorResponse {
        ErrorResponse::with_serialized_body(self.status_code(), &self)
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> Self {
        err.error_response()
    }
}

impl From<StreamEncodeError> for ErrorResponse {
    fn from(err: StreamEncodeError) -> Self {
        Self::new(err.code, err.body)
    }
}

/// What a *successful* handler return value becomes on the wire.
///
/// Implemented for any `Serialize` type (the default unary body format) and for
/// [`ResponseStream<T>`] (a stream of items). Implement it by hand for custom
/// body formats. Fallibility is not modelled here — see [`FallibleHandler`] —
/// because a blanket `impl<T: Serialize>` and an `impl … for Result` cannot
/// coexist under Rust's coherence rules.
pub trait Responder {
    fn respond(self) -> Result<RpcOutcome, ErrorResponse>;
}

impl<T> Responder for T
where
    T: Serialize,
{
    fn respond(self) -> Result<RpcOutcome, ErrorResponse> {
        let payload =
            postcard::to_allocvec(&self).map_err(|e| Error::Serialization(e.to_string()))?;

        Ok(RpcOutcome::Unary(RpcResponse { payload }))
    }
}

/// A stream of response items returned by server- and bidirectional-streaming
/// handlers. Each item is serialized to a `StreamItem` frame; a yielded `Err`
/// terminates the stream with a `StreamError`.
///
/// Handlers rarely name this type: the `#[rpc]` macro recognizes a handler that
/// returns `impl Stream<Item = T>` or `impl Stream<Item = Result<T, E>>` (or a
/// generic bound by `Stream`) and wraps the returned stream into a
/// `ResponseStream` via [`from_items`](Self::from_items) /
/// [`from_results`](Self::from_results). It stays public as the manual escape
/// hatch and the canonical carrier the wrappers build.
pub struct ResponseStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<Vec<u8>, ErrorResponse>> + Send>>,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> ResponseStream<T>
where
    T: StreamEncode + Send + 'static,
{
    /// Wraps a stream of framework-`Result` items. Equivalent to
    /// [`from_results`](Self::from_results) specialized to [`Error`].
    pub fn new(stream: impl Stream<Item = crate::Result<T>> + Send + 'static) -> Self {
        Self::from_results(stream)
    }

    /// Wraps a stream of infallible items: every item is a value to be sent,
    /// framed through the item's [`StreamEncode`] codec. This is the shape a
    /// handler returning `impl Stream<Item = T>` produces, so a stream straight
    /// from a database SDK works without per-item wrapping.
    pub fn from_items<S>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
    {
        Self {
            inner: Box::pin(T::into_frames(stream).map(|frame| frame.map_err(ErrorResponse::from))),
            _marker: std::marker::PhantomData,
        }
    }

    /// Wraps a stream of fallible items keyed on any [`ResponseError`]: a yielded
    /// `Err` terminates the stream with a `StreamError`, leaving the user free to
    /// choose their own item error type rather than the framework's. `Ok` items are
    /// framed per-item through [`StreamEncode`].
    pub fn from_results<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        E: ResponseError + 'static,
    {
        let inner = stream.map(|item| match item {
            Ok(value) => value.encode().map_err(ErrorResponse::from),

            Err(e) => Err(e.error_response()),
        });

        Self {
            inner: Box::pin(inner),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Responder for ResponseStream<T> {
    fn respond(self) -> Result<RpcOutcome, ErrorResponse> {
        Ok(RpcOutcome::Stream(self.inner))
    }
}

/// An async handler whose parameters are all `FromContext` and whose return
/// type is a [`Responder`].
///
/// `Args` is a marker for the parameter-tuple type, which lets the compiler
/// select the right arity impl (the same trick axum uses).
pub trait Handler<Args>: Send + Sized + 'static {
    fn call(
        self,
        ctx: RpcCallContext,
    ) -> impl Future<Output = Result<RpcOutcome, ErrorResponse>> + Send + 'static;
}

macro_rules! impl_handler {
    ( $($ty:ident),* ) => {
        impl<F, Fut, Res, $($ty,)*> Handler<($($ty,)*)> for F
        where
            F: Fn($($ty,)*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Res> + Send + 'static,
            Res: Responder + 'static,
            $( $ty: FromContext + Send + 'static, )*
        {
            #[allow(non_snake_case, unused_variables)]
            async fn call(self, ctx: RpcCallContext) -> Result<RpcOutcome, ErrorResponse> {
                $( let $ty = <$ty as FromContext>::from_context(&ctx).await?; )*

                let output = (self)($($ty,)*).await;

                output.respond()
            }
        }
    };
}

/// The fallible counterpart to [`Handler`]: an async fn returning
/// `Result<R, E>` where `R: Responder` and `E: ResponseError`.
///
/// A `Result` return cannot go through [`Handler`] (it is not a `Responder`),
/// so this trait carries it instead — mapping `Ok` through the responder and
/// `Err` to a transport error. Both traits erase to the same `RpcHandler` fn
/// pointer, so this split is invisible past dispatch.
pub trait FallibleHandler<Args>: Send + Sized + 'static {
    fn call(
        self,
        ctx: RpcCallContext,
    ) -> impl Future<Output = Result<RpcOutcome, ErrorResponse>> + Send + 'static;
}

macro_rules! impl_fallible_handler {
    ( $($ty:ident),* ) => {
        impl<F, Fut, Res, Err, $($ty,)*> FallibleHandler<($($ty,)*)> for F
        where
            F: Fn($($ty,)*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = core::result::Result<Res, Err>> + Send + 'static,
            Res: Responder + 'static,
            Err: ResponseError + 'static,
            $( $ty: FromContext + Send + 'static, )*
        {
            #[allow(non_snake_case, unused_variables)]
            async fn call(self, ctx: RpcCallContext) -> Result<RpcOutcome, ErrorResponse> {
                $( let $ty = <$ty as FromContext>::from_context(&ctx).await?; )*

                match (self)($($ty,)*).await {
                    Ok(value) => value.respond(),
                    Err(e) => Err(e.error_response()),
                }
            }
        }
    };
}

macro_rules! impl_params {
    ($handler:ident) => {
        $handler!();
        $handler!(T1);
        $handler!(T1, T2);
        $handler!(T1, T2, T3);
        $handler!(T1, T2, T3, T4);
        $handler!(T1, T2, T3, T4, T5);
        $handler!(T1, T2, T3, T4, T5, T6);
        $handler!(T1, T2, T3, T4, T5, T6, T7);
        $handler!(T1, T2, T3, T4, T5, T6, T7, T8);
        $handler!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
        $handler!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
    };
}

impl_params!(impl_handler);
impl_params!(impl_fallible_handler);

/// Adapts an infallible typed [`Handler`] into the erased future the router
/// invokes.
///
/// A `#[rpc]` proc macro emits a tiny non-capturing wrapper fn around this (or
/// [`dispatch_fallible`]) per handler; because it captures nothing, the wrapper
/// coerces to the `RpcHandler` fn pointer stored in a `static` `RpcDescriptor`.
pub fn dispatch_with<H, Args>(
    handler: H,
    ctx: RpcCallContext,
) -> Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>>
where
    H: Handler<Args>,
{
    Box::pin(handler.call(ctx))
}

/// Adapts a [`FallibleHandler`] (a `Result`-returning handler) into the erased
/// future the router invokes. Counterpart to [`dispatch_with`].
pub fn dispatch_fallible<H, Args>(
    handler: H,
    ctx: RpcCallContext,
) -> Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>>
where
    H: FallibleHandler<Args>,
{
    Box::pin(handler.call(ctx))
}

#[cfg(test)]
mod tests {
    use serde::Serializer;
    use serde::ser::Error as _;
    use upwell_transport::PredefinedCode;

    use super::*;

    /// A type whose serialization always fails, to exercise the FR-011 fallback.
    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("intentional serialization failure"))
        }
    }

    #[test]
    fn body_serialization_failure_preserves_code_with_empty_body() {
        // FR-011: a body that fails to serialize yields the intended code with a
        // fallback (empty) body rather than dropping the code or panicking.
        let code = StatusCode::from(PredefinedCode::NotFound);
        let response = ErrorResponse::with_serialized_body(code, &Unserializable);

        assert_eq!(response.code, code);
        assert!(response.body.is_empty());
    }

    #[test]
    fn serializable_error_uses_blanket_impl() {
        // A `Serialize` error gets the blanket `ResponseError`: the default
        // `Internal` code, with the error value itself as the body.
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct PlainError {
            reason: String,
        }

        let error = PlainError {
            reason: "boom".to_string(),
        };
        let response = error.error_response();
        let decoded: PlainError = postcard::from_bytes(&response.body).expect("decode body");

        assert_eq!(decoded.reason, "boom");
        assert_eq!(response.code.predefined(), PredefinedCode::Internal);
    }
}
