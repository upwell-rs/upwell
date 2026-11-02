//! Whether a request copies its headers into `RequestMeta`.
//!
//! Middleware puts a header value sharing [`PROBE`]'s bytes on the request. Once [`Copied`] has
//! removed the request's own value, [`PROBE`] is unique exactly when nothing copied the headers.

use std::sync::LazyLock;

use upwell::axum::axum::body::Bytes;
use upwell::axum::axum::extract::FromRequestParts;
use upwell::axum::axum::http::request::Parts;
use upwell::axum::axum::middleware::from_fn;

use super::*;

static PROBE: LazyLock<Bytes> = LazyLock::new(|| Bytes::from(b"probe".to_vec()));

/// Whether something copied the headers, read after the extractors before it. It removes the
/// request's own value first. The scope lives in the request's extensions, which axum drops
/// before a handler's body runs, so the check has to happen during extraction.
struct Copied(bool);

impl<S: Send + Sync> FromRequestParts<S> for Copied {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.headers.remove("x-probe");

        Ok(Self(!PROBE.is_unique()))
    }
}

/// Routes reporting whether their request copied the headers.
#[controller(path = "/seeds")]
struct SeedController;

#[handlers]
impl SeedController {
    #[get("/unrelated")]
    async fn unrelated(
        &self,
        Inject(_unrelated): Inject<Arc<Unrelated>>,
        Copied(copied): Copied,
    ) -> Json<bool> {
        Json(copied)
    }

    #[get("/handle")]
    async fn handle(&self, _handle: ScopeHandle, Copied(copied): Copied) -> Json<bool> {
        Json(copied)
    }

    #[get("/reader")]
    async fn reader(
        &self,
        Inject(_reader): Inject<Arc<Reader>>,
        Copied(copied): Copied,
    ) -> Json<bool> {
        Json(copied)
    }
}

async fn probe(mut request: RawRequest, next: Next) -> Response {
    let value = HeaderValue::from_maybe_shared(PROBE.clone()).expect("the probe is a valid value");

    request.headers_mut().insert("x-probe", value);

    next.run(request).await
}

async fn copied(router: &Router, uri: &str) -> bool {
    json(get(router, uri).await).await
}

#[tokio::test]
async fn only_a_route_reading_the_seed_copies_the_headers() {
    let app = upwell::App::<Axum>::builder("test-request-seeds")
        .controller::<SeedController>()
        .component::<Reader>()
        .component::<Unrelated>()
        .layer(from_fn(probe))
        .build()
        .await
        .expect("app builds");
    let router = app.protocol().router().clone();

    assert!(
        !copied(&router, "/seeds/unrelated").await,
        "no read, no copy"
    );
    assert!(copied(&router, "/seeds/reader").await);
    assert!(
        copied(&router, "/seeds/handle").await,
        "a handle may read anything later, so handing it out copies"
    );
}
