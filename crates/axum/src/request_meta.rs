//! Native HTTP request data, seeded into every request scope.
//!
//! Mirrors how the RPC protocol seeds `PeerInfo` into its connection scope: a factory-less,
//! by-value [`ComponentDescriptor`] whose instance is supplied at scope-open time rather than
//! constructed, so a request-scoped component (or a handler, via [`Inject`](crate::Inject))
//! can depend on the request's method, URI, headers, and cookies without axum's own
//! extractors ever entering the DI graph.

use std::collections::HashMap;

use axum::http::{HeaderMap, Method, Uri};
use upwell_core::{Scope, TypeDescriptor};
use upwell_di::{ComponentDescriptor, Injectable};

use crate::scope::HttpRequest as HttpRequestScope;

/// Native request data available to request-scoped DI components and handlers.
#[derive(Clone)]
pub struct RequestMeta {
    /// The request's HTTP method.
    pub method: Method,

    /// The request's URI.
    pub uri: Uri,

    /// The request's headers.
    pub headers: HeaderMap,

    /// Cookies parsed from the `Cookie` header, keyed by name. Empty if the request sent no
    /// `Cookie` header, or if a cookie pair failed to parse.
    pub cookies: HashMap<String, String>,
}

impl RequestMeta {
    /// Builds a [`RequestMeta`] from the parts of an incoming request, parsing any `Cookie`
    /// header into name/value pairs.
    pub fn from_parts(method: Method, uri: Uri, headers: HeaderMap) -> Self {
        let cookies = headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(cookie::Cookie::split_parse)
            .filter_map(Result::ok)
            .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
            .collect();

        Self {
            method,
            uri,
            headers,
            cookies,
        }
    }
}

/// A request-scoped, by-value injectable (mirroring the RPC protocol's `PeerInfo`): a
/// component depends on it directly as `meta: RequestMeta`, no `Arc`. Cheap enough to clone —
/// a `HeaderMap`/cookie map bounded by one request's actual headers.
impl Injectable for RequestMeta {
    type Target = RequestMeta;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[cfg(feature = "di-check")]
impl upwell_di::Provided<RequestMeta> for upwell_di::Wiring {}

/// The framework-provided request-scoped injectable for the incoming request's native data.
///
/// Seeded into every request scope with the actual [`RequestMeta`], so a request-scoped
/// component can depend on it directly (e.g. to read a bearer token or a session cookie in
/// its constructor).
pub(crate) static REQUEST_META_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    "__upwell_request_meta",
    "RequestMeta",
    TypeDescriptor::of::<RequestMeta>("RequestMeta"),
    HttpRequestScope::KEY,
);

#[cfg(test)]
mod tests;
