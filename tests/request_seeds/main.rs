#![cfg(feature = "axum")]

//! End-to-end tests for when an HTTP request captures its head into `RequestMeta`, and which
//! head each reader sees.
//!
//! Every request starts with `x-tenant: original`. Global middleware may rewrite it, open the
//! request scope, or read a component built on `RequestMeta` and report the tenant it saw in the
//! `x-seen` response header.

mod copies;
mod order;
mod strict;

use std::sync::Arc;

use upwell::axum::axum::Router;
use upwell::axum::axum::body::Body;
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::{HeaderValue, StatusCode};
use upwell::axum::axum::middleware::Next;
use upwell::axum::axum::response::Response;
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{Axum, RequestMeta, ScopeHandle};
use upwell::{AppBuilder, component, methods};

/// The header every test request carries and middleware rewrites.
const TENANT: &str = "x-tenant";

/// The response header middleware reports the tenant it saw in.
const SEEN: &str = "x-seen";

/// A request-scoped component holding the tenant of the head it was built from.
#[component(scope = HttpRequest)]
struct Reader {
    #[default]
    tenant: Option<String>,
}

#[methods]
impl Reader {
    #[init]
    async fn init(meta: RequestMeta) -> Self {
        Self {
            tenant: tenant_of(&meta),
        }
    }
}

/// A request-scoped component that does not read the head.
#[component(scope = HttpRequest)]
struct Unrelated {}

/// Routes declaring a read of the head, or only a component that does not read it.
#[controller(path = "/tenant")]
struct TenantController;

#[handlers]
impl TenantController {
    #[get("/declared")]
    async fn declared(&self, Inject(reader): Inject<Arc<Reader>>) -> Json<Option<String>> {
        Json(reader.tenant.clone())
    }

    #[get("/undeclared")]
    async fn undeclared(&self, Inject(_unrelated): Inject<Arc<Unrelated>>) {}
}

fn tenant_of(meta: &RequestMeta) -> Option<String> {
    meta.headers
        .get(TENANT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// An application serving [`TenantController`] with both request-scoped components.
fn app() -> AppBuilder<Axum> {
    upwell::App::<Axum>::builder("test-request-seeds")
        .controller::<TenantController>()
        .component::<Reader>()
        .component::<Unrelated>()
}

async fn router(builder: AppBuilder<Axum>) -> Router {
    let app = builder.build().await.expect("app builds");

    app.protocol().router().clone()
}

/// Sends `GET uri` with the original tenant.
async fn get(router: &Router, uri: &str) -> Response {
    let request = RawRequest::builder()
        .uri(uri)
        .header(TENANT, "original")
        .body(Body::empty())
        .expect("the request is valid");

    router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

/// The tenant middleware reported seeing, if it reported one.
fn seen(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(SEEN)
        .map(|value| value.to_str().expect("the reported tenant is text"))
}

/// The JSON body of a successful `response`.
async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);

    let body = upwell::axum::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads");

    serde_json::from_slice(&body).expect("the body decodes")
}

/// The tenant the `/tenant/declared` handler's [`Reader`] holds.
async fn declared_tenant(response: Response) -> Option<String> {
    json(response).await
}

fn report(response: &mut Response, tenant: Option<&str>) {
    let value = HeaderValue::from_str(tenant.unwrap_or("none")).expect("the tenant is a value");

    response.headers_mut().insert(SEEN, value);
}

/// Middleware rewriting the tenant to `rewritten`.
async fn rewrite(mut request: RawRequest, next: Next) -> Response {
    request
        .headers_mut()
        .insert(TENANT, HeaderValue::from_static("rewritten"));

    next.run(request).await
}

/// Middleware opening the request scope through an injection that does not read the head.
async fn open_unrelated(
    Inject(_unrelated): Inject<Arc<Unrelated>>,
    request: RawRequest,
    next: Next,
) -> Response {
    next.run(request).await
}

/// Middleware opening the request scope through a [`ScopeHandle`].
async fn open_handle(mut request: RawRequest, next: Next) -> Response {
    ScopeHandle::of(&mut request)
        .await
        .expect("the middleware opens the request scope");

    next.run(request).await
}

/// Middleware injecting [`Reader`] and reporting the tenant it holds.
async fn read_reader(
    Inject(reader): Inject<Arc<Reader>>,
    request: RawRequest,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;

    report(&mut response, reader.tenant.as_deref());

    response
}

/// Middleware injecting [`RequestMeta`] itself and reporting its tenant.
async fn read_meta(Inject(meta): Inject<RequestMeta>, request: RawRequest, next: Next) -> Response {
    let mut response = next.run(request).await;

    report(&mut response, tenant_of(&meta).as_deref());

    response
}
