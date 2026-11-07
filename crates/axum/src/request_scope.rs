//! Opening the HTTP request scope on first use, and capturing what its seed reads.
//!
//! The protocol's router runs every request inside [`RequestScopeLayer`], which only makes the
//! prepared [`ScopeOpener`] reachable for the request's task. The scope itself opens the first
//! time something injects from it ([`Inject`](crate::Inject),
//! [`ScopeHandle`](crate::ScopeHandle)), and is then kept in a private request extension, so
//! middleware and the handler share one instance. A
//! request that injects nothing request-scoped opens no scope.
//!
//! A request to a controller route opens its scope with the route's entry plan, which builds
//! only what the route injects; anything else is built when something resolves it. Other
//! requests build every slot of the scope.
//!
//! The exception is a scope with eager components, which must run for every routed request,
//! or one a `Current`/`Scoped` accessor reads, which must be open for any code of the request
//! to read it. [`open_eager_scope`] then opens the scope before global middleware runs, as RPC opens a
//! call's scope before its middleware: eager components run for a request middleware rejects
//! too, and read the request head as the client sent it. A request no route matches still opens
//! no scope on its own. The placement is the `router.layer` call for [`open_eager_scope`] in
//! `PreparedAxum::build`, inside the body limit and the request timeout.
//!
//! # When [`RequestMeta`] is captured
//!
//! Axum owns the request head and consumes it before a handler's body runs, so the seed cannot
//! be derived from it whenever something happens to read it. Copying it for every request just
//! in case is the cost this module avoids, so it is captured once per request, at the first
//! point that has to:
//!
//! 1. When the scope opens, if that opening reads it: the route's entry injects it, or builds or
//!    lazily reaches a component that depends on it; or, with no entry, the opening builds such
//!    a component. This is what a declared read sees, whatever runs after the scope opens.
//! 2. At an [`Inject`](crate::Inject) of something that reads it and the route does not declare,
//!    such as a component only middleware injects. It holds the head as it is at that point, so
//!    it includes changes middleware made since the scope opened.
//! 3. When a [`ScopeHandle`](crate::ScopeHandle) is handed out, because a handle can resolve
//!    anything at any later time. Using a handle therefore costs one capture, whether or not it
//!    reads the seed.
//!
//! Every reader of one request sees the same capture. A request that reaches none of these
//! copies nothing.
//!
//! # The invocation context
//!
//! [`RequestScopeLayer`] also runs the request, global middleware included, in its
//! [`InvocationContext`](upwell_di::context::InvocationContext). The context starts with a
//! pending request scope and no scope instance, because the scope opens later or not at all; the opening binds its container
//! into the context, so code that runs after it in the request's task, middleware and the
//! handler alike, sees the request scope. With eager components the scope opens before global
//! middleware, so all of it runs in the scope. The context wraps the router's future, and the
//! router calls a route's layered services when that future is polled, so a layered service's
//! synchronous `call` runs in it too. It ends with that future, so a response body polled after
//! it runs outside the context.

use std::collections::HashMap;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{MatchedPath, Request};
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::task::futures::TaskLocalFuture;
use tower::{Layer, Service};
use upwell_app::{Opening, ScopeEntry, ScopeOpener};
use upwell_config::Cfg;
use upwell_core::Scope;
use upwell_di::context::{Entered, TargetRef, bind_opened_scope};
use upwell_di::{FromContainer, ScopeContainer};

use crate::config::AxumConfig;
use crate::extract::InjectRejection;
use crate::request_meta::RequestMeta;
use crate::scope::HttpRequest;
use crate::target::HttpTarget;

tokio::task_local! {
    static REQUEST_SCOPES: Arc<RequestScopes>;
}

/// What every request served by the protocol's router opens its scope from.
pub(crate) struct RequestScopes {
    opener: ScopeOpener<HttpRequest>,
    routes: RouteEntries,
    config: Cfg<AxumConfig>,
}

/// The entry of each controller route, by method and the path the router matches it under.
#[derive(Default)]
pub(crate) struct RouteEntries(HashMap<Method, HashMap<Box<str>, ScopeEntry<HttpRequest>>>);

/// The open scope of a request, in its extensions. Private to this module so nothing reads it
/// without going through a path that captures what a later read of the scope needs.
#[derive(Clone)]
struct OpenedScope(Arc<ScopeContainer>);

/// The head of a request, borrowed until something reads [`RequestMeta`].
#[derive(Clone, Copy)]
struct Head<'a> {
    method: &'a Method,
    uri: &'a Uri,
    headers: &'a HeaderMap,
}

/// What opening a request's scope builds when no route entry says.
#[derive(Clone, Copy)]
enum WithoutEntry {
    /// Every slot of the scope.
    Everything,
    /// Only the eager set; the rest is built when something resolves it.
    Eager,
}

/// Makes [`RequestScopes`] reachable for each request it serves, so its scope opens on the
/// first injection and request-scoped components can depend on the native request without
/// axum's own extractors entering the DI graph. A request that injects nothing opens no scope,
/// unless the scope has eager components.
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

impl<'a> Head<'a> {
    fn of_parts(parts: &'a Parts) -> Self {
        Self {
            method: &parts.method,
            uri: &parts.uri,
            headers: &parts.headers,
        }
    }

    fn of_request(request: &'a Request) -> Self {
        Self {
            method: request.method(),
            uri: request.uri(),
            headers: request.headers(),
        }
    }

    fn capture(self) -> RequestMeta {
        RequestMeta::from_parts(self.method.clone(), self.uri.clone(), self.headers.clone())
    }
}

impl RequestScopeLayer {
    pub(crate) fn new(
        opener: ScopeOpener<HttpRequest>,
        routes: RouteEntries,
        config: Cfg<AxumConfig>,
    ) -> Self {
        Self {
            scopes: Arc::new(RequestScopes {
                opener,
                routes,
                config,
            }),
        }
    }
}

impl RouteEntries {
    pub(crate) fn insert(&mut self, method: Method, path: String, entry: ScopeEntry<HttpRequest>) {
        self.0
            .entry(method)
            .or_default()
            .insert(path.into_boxed_str(), entry);
    }

    /// The entry of the route serving `method` at `path`. A `HEAD` request without a route of
    /// its own is served by the `GET` route, so it uses that entry.
    fn get(&self, method: &Method, path: &str) -> Option<&ScopeEntry<HttpRequest>> {
        let routed = self.0.get(method).and_then(|paths| paths.get(path));

        match (routed, method) {
            (None, &Method::HEAD) => self.0.get(&Method::GET)?.get(path),
            (routed, _) => routed,
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
    type Future = TaskLocalFuture<Arc<RequestScopes>, Entered<S::Future>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let context = self
            .scopes
            .opener
            .context(TargetRef::from_static(&HttpTarget::Request))
            .with_pending_scope(HttpRequest::KEY, None);

        REQUEST_SCOPES.scope(
            Arc::clone(&self.scopes),
            context.enter(self.inner.call(request)),
        )
    }
}

/// The protocol's configuration, for a request served by the protocol's router.
pub(crate) fn config() -> Option<Cfg<AxumConfig>> {
    REQUEST_SCOPES.try_with(|scopes| scopes.config.clone()).ok()
}

/// The request's scope, opened on first use from the request head in `parts`.
async fn of_parts(parts: &mut Parts) -> Result<Arc<ScopeContainer>, InjectRejection> {
    if let Some(scope) = opened(&parts.extensions) {
        return Ok(scope);
    }

    let matched = parts.extensions.get::<MatchedPath>();
    let scope = open(Head::of_parts(parts), matched, WithoutEntry::Everything).await?;

    parts.extensions.insert(OpenedScope(Arc::clone(&scope)));

    Ok(scope)
}

/// The scope of `request`, opened on first use.
async fn of_request(request: &mut Request) -> Result<Arc<ScopeContainer>, InjectRejection> {
    if let Some(scope) = opened(request.extensions()) {
        return Ok(scope);
    }

    open_request(request, WithoutEntry::Everything).await
}

/// Resolves `H` from the request's scope, opened on first use. When `H` reads a [`RequestMeta`]
/// nothing has captured yet, it is captured here, from the head as `parts` holds it now.
pub(crate) async fn inject<H: FromContainer>(parts: &mut Parts) -> Result<H, InjectRejection> {
    let scope = of_parts(parts).await?;
    let head = Head::of_parts(parts);

    scope
        .extract_seeding::<H, RequestMeta>(|| head.capture())
        .await
        .map_err(|source| InjectRejection::Unresolved {
            type_name: std::any::type_name::<H>(),
            source,
        })
}

/// The request's scope for its [`ScopeHandle`] extractor. The handle resolves anything at any
/// time, also after axum has consumed `parts`, so [`RequestMeta`] is captured now.
///
/// [`ScopeHandle`]: crate::ScopeHandle
pub(crate) async fn handle_of_parts(
    parts: &mut Parts,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    let scope = of_parts(parts).await?;

    capture_into(&scope, Head::of_parts(parts))?;

    Ok(scope)
}

/// The scope of `request` for [`ScopeHandle::of`](crate::ScopeHandle::of), with [`RequestMeta`]
/// captured as [`handle_of_parts`] does.
pub(crate) async fn handle_of_request(
    request: &mut Request,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    let scope = of_request(request).await?;

    capture_into(&scope, Head::of_request(request))?;

    Ok(scope)
}

fn capture_into(scope: &ScopeContainer, head: Head<'_>) -> Result<(), InjectRejection> {
    scope
        .ensure_seed(|| head.capture())
        .map_err(InjectRejection::SeedFailed)
}

/// Middleware opening a routed request's scope, unless something has already. Without an entry
/// it builds only the eager set, so a route that injects nothing does not pay for the rest of
/// the scope.
///
/// The protocol installs it only when the application registers an eager request-scoped
/// component or a component with a `Current`/`Scoped` accessor, so other applications do not
/// pay for the layer, and it opens the scope only while the current generation builds an eager
/// component or an accessor reads the scope. It is applied after the global
/// middleware, so it runs before all of it, as RPC opens a call's scope before its middleware,
/// and inside the request timeout, which covers eager construction.
pub(crate) async fn open_eager_scope(mut request: Request, next: Next) -> Response {
    let routed = request.extensions().get::<MatchedPath>().is_some();
    let at_once = REQUEST_SCOPES
        .try_with(|scopes| scopes.opener.opens_at_once())
        .unwrap_or(false);

    if routed
        && at_once
        && opened(request.extensions()).is_none()
        && let Err(rejection) = open_request(&mut request, WithoutEntry::Eager).await
    {
        return rejection.into_response();
    }

    next.run(request).await
}

fn opened(extensions: &Extensions) -> Option<Arc<ScopeContainer>> {
    extensions
        .get::<OpenedScope>()
        .map(|OpenedScope(scope)| Arc::clone(scope))
}

async fn open_request(
    request: &mut Request,
    without_entry: WithoutEntry,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    let matched = request.extensions().get::<MatchedPath>();
    let scope = open(Head::of_request(request), matched, without_entry).await?;

    request
        .extensions_mut()
        .insert(OpenedScope(Arc::clone(&scope)));

    Ok(scope)
}

/// Opens the request's scope, capturing [`RequestMeta`] from `head` when the opening reads it.
async fn open(
    head: Head<'_>,
    matched: Option<&MatchedPath>,
    without_entry: WithoutEntry,
) -> Result<Arc<ScopeContainer>, InjectRejection> {
    let scopes = REQUEST_SCOPES
        .try_with(Arc::clone)
        .map_err(|_| InjectRejection::MissingScope)?;
    let entry = matched.and_then(|path| scopes.routes.get(head.method, path.as_str()));
    let opening = match (entry, without_entry) {
        (Some(entry), _) => Opening::Entry(entry),
        (None, WithoutEntry::Everything) => Opening::Everything,
        (None, WithoutEntry::Eager) => Opening::OnDemand,
    };

    let opened = scopes
        .opener
        .builder(HttpRequest)
        .open_with_bound_seed(opening, || head.capture())
        .await
        .map_err(InjectRejection::ScopeFailed)?;
    let container = opened.into_container();

    bind_opened_scope(&container);

    Ok(container)
}
