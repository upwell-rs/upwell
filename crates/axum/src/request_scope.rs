//! Opening the HTTP request scope on first use.
//!
//! The protocol's router runs every request inside [`RequestScopeLayer`], which only makes the
//! prepared [`ScopeOpener`] reachable for the request's task. The scope itself opens the first
//! time something injects from it ([`Inject`](crate::Inject), [`ScopeHandle`]), and is then
//! kept in the request extensions, so middleware and the handler share one instance. A
//! request that injects nothing request-scoped opens no scope.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::Extensions;
use axum::http::request::Parts;
use tokio::task::futures::TaskLocalFuture;
use tower::{Layer, Service};
use upwell_app::ScopeOpener;
use upwell_config::Cfg;
use upwell_di::ScopeContainer;

use crate::config::AxumConfig;
use crate::extract::{InjectRejection, ScopeHandle};
use crate::request_meta::RequestMeta;
use crate::scope::HttpRequest;

tokio::task_local! {
    static REQUEST_SCOPES: Arc<RequestScopes>;
}

/// What every request served by the protocol's router opens its scope from.
pub(crate) struct RequestScopes {
    opener: ScopeOpener<HttpRequest>,
    config: Cfg<AxumConfig>,
}

/// Makes [`RequestScopes`] reachable for each request it serves.
#[derive(Clone)]
pub(crate) struct RequestScopeLayer {
    scopes: Arc<RequestScopes>,
}

/// The service [`RequestScopeLayer`] wraps around the router's routes.
#[derive(Clone)]
pub(crate) struct RequestScopeService<S> {
    inner: S,
    scopes: Arc<RequestScopes>,
}

impl RequestScopeLayer {
    pub(crate) fn new(opener: ScopeOpener<HttpRequest>, config: Cfg<AxumConfig>) -> Self {
        Self {
            scopes: Arc::new(RequestScopes { opener, config }),
        }
    }
}

impl<S> Layer<S> for RequestScopeLayer {
    type Service = RequestScopeService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestScopeService {
            inner,
            scopes: Arc::clone(&self.scopes),
        }
    }
}

impl<S: Service<Request>> Service<Request> for RequestScopeService<S> {
    type Response = S::Response;
    type Error = S::Error;
    type Future = TaskLocalFuture<Arc<RequestScopes>, S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        REQUEST_SCOPES.scope(Arc::clone(&self.scopes), self.inner.call(request))
    }
}

/// The protocol's configuration, for a request served by the protocol's router.
pub(crate) fn config() -> Option<Cfg<AxumConfig>> {
    REQUEST_SCOPES.try_with(|scopes| scopes.config.clone()).ok()
}

/// The request's scope, opened on first use from the request head in `parts`.
pub(crate) async fn of_parts(parts: &mut Parts) -> Result<Arc<ScopeContainer>, InjectRejection> {
    if let Some(scope) = opened(&parts.extensions) {
        return Ok(scope);
    }

    let meta = RequestMeta::from_parts(
        parts.method.clone(),
        parts.uri.clone(),
        parts.headers.clone(),
    );

    open(meta, &mut parts.extensions).await
}

/// The scope of `request`, opened on first use.
pub(crate) async fn of_request(
    request: &mut Request,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    if let Some(scope) = opened(request.extensions()) {
        return Ok(scope);
    }

    let meta = RequestMeta::from_parts(
        request.method().clone(),
        request.uri().clone(),
        request.headers().clone(),
    );

    open(meta, request.extensions_mut()).await
}

fn opened(extensions: &Extensions) -> Option<Arc<ScopeContainer>> {
    extensions
        .get::<ScopeHandle>()
        .map(|ScopeHandle(scope)| Arc::clone(scope))
}

/// Opens the request's scope seeded with `meta` and stores it in `extensions`.
///
/// The seed reflects the request head when the scope opens, so it carries changes middleware
/// made before the first injection.
async fn open(
    meta: RequestMeta,
    extensions: &mut Extensions,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    let scopes = REQUEST_SCOPES
        .try_with(Arc::clone)
        .map_err(|_| InjectRejection::MissingScope)?;
    let scope = scopes
        .opener
        .builder()
        .seed(meta)
        .open_default()
        .await
        .map_err(|error| InjectRejection::ScopeFailed(error.to_string()))?
        .into_container();

    extensions.insert(ScopeHandle(Arc::clone(&scope)));

    Ok(scope)
}
