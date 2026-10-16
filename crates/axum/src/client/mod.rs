//! The generated HTTP client's runtime: the request body family, the response envelope, and
//! pluggable transport backends.
//!
//! A generated `{Controller}Client<C>` is transport-generic: `C` is any backend implementing
//! the [`Unary`](upwell_client::Unary) capability with `Request<B> = http::Request<B>` and
//! `Response<R> = HttpResponse<R>` (plus the [`Encodes`](upwell_transport::Encodes) /
//! [`Decodes`](upwell_transport::Decodes) codec). Both bundled backends — [`reqwest`] and
//! `hyper` — qualify, so the same client runs over either; pick one with the matching feature.

mod body;
// The shared browser-client `Connection` (wasm-only; needs the reqwest fetch backend).
#[cfg(all(
    target_family = "wasm",
    any(feature = "reqwest", feature = "tungstenite")
))]
mod connection;
mod headers;
mod interceptor;
// The protocol-generic pub/sub client capabilities (message send/request, topic subscribe). Behind
// `ws` (not `stomp`), so a non-STOMP protocol's client reuses them.
#[cfg(all(feature = "ws", feature = "client"))]
mod messaging;
mod response;
mod streaming;
// Shared ws runtime (task spawn) for the client transports; only needed with a ws transport.
#[cfg(all(feature = "tungstenite", feature = "client"))]
pub mod ws_rt;
// The protocol-neutral correlated request/reply WebSocket actor.
#[cfg(all(feature = "ws", feature = "client"))]
mod websocket;

#[cfg(all(feature = "hyper", not(target_family = "wasm")))]
mod hyper_backend;
#[cfg(feature = "reqwest")]
mod reqwest_backend;

pub use body::{Form, HttpBody, Json, Multipart, OctetStream, RawForm};
#[cfg(all(
    target_family = "wasm",
    any(feature = "reqwest", feature = "tungstenite")
))]
pub use connection::Connection;
pub use headers::RequestHeaders;
#[cfg(all(target_family = "wasm", feature = "reqwest"))]
pub use interceptor::WasmClientInterceptor;
pub use interceptor::{ClientInterceptor, DefaultClientInterceptor};
#[cfg(all(feature = "ws", feature = "client"))]
pub use messaging::*;
pub use response::{HttpResponse, RedirectResponse};
pub use streaming::{HttpClientStreaming, HttpStreaming, StreamDecode, encode_stream};
#[cfg(all(feature = "ws", feature = "client"))]
pub use websocket::*;

/// Re-exported so generated streaming-client code names the codec without a separate dep.
pub use upwell_transport::{Decodes, Encodes};

/// Raw unary HTTP exchange used by generated status-aware route clients.
pub trait HttpExchange: upwell_client::Transport<Status = http::StatusCode> {
    /// Performs one request without classifying statuses or decoding the body.
    fn exchange<B>(
        &self,
        request: http::Request<B>,
    ) -> impl std::future::Future<
        Output = Result<HttpResponse<Vec<u8>>, upwell_client::ClientError<http::StatusCode>>,
    > + upwell_client::MaybeSend
    where
        Self: Encodes<B>,
        B: upwell_client::MaybeSend;

    /// Decodes one already-buffered response body using this backend's codec.
    fn decode_response<T>(
        &self,
        body: Vec<u8>,
    ) -> Result<T, upwell_client::ClientError<http::StatusCode>>
    where
        Self: Decodes<T>;

    /// Classifies an undeclared response status and reports it through this backend's interceptor.
    fn fail_unexpected<E>(
        &self,
        response: HttpResponse<Vec<u8>>,
    ) -> upwell_client::ClientError<http::StatusCode, E>;
}

/// Percent-encodes one URI path segment according to RFC 3986. Generated clients call this for
/// every route `Path<T>` substitution before building the request URI.
pub fn encode_path_segment(value: impl std::fmt::Display) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let value = value.to_string();
    let mut out = String::with_capacity(value.len());

    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }

    out
}

/// Percent-encodes a catch-all URI path value one segment at a time, preserving `/` separators.
/// Generated clients use this only for `{*path}` holes; ordinary `{id}` holes continue to use
/// [`encode_path_segment`] so a slash can never escape a single route segment.
pub fn encode_path_segments(value: impl std::fmt::Display) -> String {
    value
        .to_string()
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// URL-encodes a typed `Query<T>` value into a query string (without the leading `?`). Generated
/// clients call this for a route's `Query<T>` param when building the request URI. Like path
/// substitution, it is a "valid by construction" step in the infallible URI builder: a `Dto` query
/// type serializes cleanly, so an encoder error (a shape `serde_urlencoded` rejects, e.g. a nested
/// struct) surfaces as a panic rather than a checked error on every call.
pub fn encode_query<T: serde::Serialize>(value: &T) -> String {
    serde_urlencoded::to_string(value).expect("query value serializes to a URL-encoded string")
}

/// Maps a non-success HTTP response into a [`ClientError::Remote`](upwell_client::ClientError),
/// carrying the genuine [`http::StatusCode`](axum::http::StatusCode) and the raw error body. The
/// HTTP client's protocol status *is* the HTTP status — no folding into the RPC packed status — so
/// a caller inspects `error.code()` as an `http::StatusCode` directly. Used by the streaming
/// transports, where a pre-stream failure has no response envelope to surface the status on (it is
/// the outer `Result`'s `Err`).
#[cfg(any(feature = "reqwest", feature = "hyper"))]
pub(crate) fn remote_error(
    status: http::StatusCode,
    body: Vec<u8>,
) -> upwell_client::ClientError<http::StatusCode> {
    upwell_client::ClientError::Remote(upwell_client::ErrorBody::new(status, body))
}

#[cfg(any(feature = "reqwest", feature = "hyper"))]
pub(crate) fn redirect_error<E>(
    status: http::StatusCode,
    headers: &http::HeaderMap,
    body: Vec<u8>,
) -> upwell_client::ClientError<http::StatusCode, E> {
    upwell_client::ClientError::Redirect {
        status,
        location: headers
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body,
    }
}

#[cfg(any(
    feature = "reqwest",
    all(feature = "hyper", not(target_family = "wasm"))
))]
fn unexpected_response<E>(
    response: HttpResponse<Vec<u8>>,
) -> upwell_client::ClientError<http::StatusCode, E> {
    let (status, headers, body) = response.into_parts();

    if status.is_redirection() {
        return upwell_client::ClientError::Redirect {
            status,
            location: headers
                .get(http::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body,
        };
    }

    upwell_client::ClientError::Remote(upwell_client::ErrorBody::new(status, body))
}

#[cfg(all(feature = "hyper", not(target_family = "wasm")))]
pub use hyper_backend::HyperClient;
#[cfg(feature = "reqwest")]
pub use reqwest_backend::ReqwestClient;

#[cfg(test)]
mod tests;
