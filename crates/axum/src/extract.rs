//! The DI↔axum bridge: a per-request scope handle and the [`Inject`] extractor.
//!
//! The integration surface between the framework's dependency injection and axum is a
//! single [`ScopeHandle`] — an `Arc` of the request's
//! [`ScopeContainer`](upwell_di::ScopeContainer). The first [`Inject`] or [`ScopeHandle`]
//! extraction of a request opens its scope; later ones reuse it, and
//! [`Inject`] resolves a component through the scope chain. Native axum extractors (`Json`, `Path`, `Query`, `State`, …)
//! are untouched and compose with [`Inject`] in the same handler signature.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use upwell_di::{FromContainer, ScopeContainer};

/// The per-request DI scope, for dynamic access.
///
/// A request's scope opens on its first [`Inject`] or [`ScopeHandle`] extraction. Middleware
/// holding the whole request reaches it with [`ScopeHandle::of`], and it is not a request
/// extension of its own. Cloning is cheap — it is an `Arc` bump.
///
/// A handle can resolve anything at any later time, including after axum has consumed the
/// request head, so getting one copies the head into the request's
/// [`RequestMeta`](crate::RequestMeta) if nothing has yet. Prefer [`Inject`] where the type is
/// known: it copies the head only when what it injects reads it.
#[derive(Clone)]
pub struct ScopeHandle(pub Arc<ScopeContainer>);

impl ScopeHandle {
    /// The scope of `request`, opened on first use.
    ///
    /// # Errors
    ///
    /// As the [`ScopeHandle`] extractor.
    pub async fn of(request: &mut Request) -> Result<Self, InjectRejection> {
        crate::request_scope::handle_of_request(request)
            .await
            .map(Self)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ScopeHandle {
    type Rejection = InjectRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        crate::request_scope::handle_of_parts(parts).await.map(Self)
    }
}

/// Injects anything a constructor parameter can be — any [`FromContainer`] — from the
/// request's scope: a component handle (`Arc<T>`, `Dep<T>`, a by-value injectable), every
/// provider of a trait (`Vec<Arc<dyn T>>` / `HashMap<String, Arc<dyn T>>`), an optional
/// component (`Option<Arc<T>>`), or a resolver-backed value such as `Cfg<T>`. Resolution
/// walks the request → singleton scope chain and constructs a fresh instance when the target
/// is a `Transient`.
///
/// This is how a route reaches request-scoped and transient components: the route runs
/// inside the request scope, so injection can reach down to shorter-lived components that a
/// singleton controller cannot hold as a field. The controller itself arrives through
/// `&self`, resolved once when its router is built.
pub struct Inject<H>(pub H);

/// Why an [`Inject`] or [`ScopeHandle`] extraction failed. Always a server-side
/// misconfiguration: it answers an empty `500` and logs the typed cause.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InjectRejection {
    /// The request is not served by the protocol's router, so it has no scope to open.
    #[error("request scope unavailable; the request is not served by the Axum protocol's router")]
    MissingScope,

    /// The request's scope failed to open.
    #[error("the request scope failed to open")]
    ScopeFailed(#[source] upwell_app::Error),

    /// The request head could not be seeded into the request scope.
    #[error("the request head could not be seeded into the request scope")]
    SeedFailed(#[source] upwell_di::Error),

    /// The requested type could not be extracted from the container (no such component or
    /// provider, or a resolver-backed value like `Cfg<T>` was unavailable).
    #[error("failed to inject `{type_name}`")]
    Unresolved {
        /// The type that was being injected.
        type_name: &'static str,
        /// The underlying DI error.
        #[source]
        source: upwell_di::Error,
    },
}

const _: () = {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    assert_send_sync_static::<InjectRejection>();
};

impl IntoResponse for InjectRejection {
    fn into_response(self) -> Response {
        tracing::error!(
            target: "upwell::axum",
            error = &self as &(dyn std::error::Error + 'static),
            "request scope extraction failed"
        );

        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

impl<S, H> FromRequestParts<S> for Inject<H>
where
    S: Send + Sync,
    H: FromContainer,
{
    type Rejection = InjectRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        crate::request_scope::inject::<H>(parts).await.map(Inject)
    }
}

#[cfg(test)]
mod tests;
