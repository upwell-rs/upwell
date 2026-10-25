#![cfg(feature = "axum")]

//! End-to-end tests for axum middleware registration (global/controller/path) and the
//! `RequestMeta` request-scope seed.
//!
//! Covers: a raw `axum::middleware::from_fn` closure (standard, unmodified ecosystem
//! middleware) and a DI-backed `AxumMiddleware` interleaving in the documented order across
//! all three tiers; the same middleware type resolved at two attach points sharing one
//! instance instead of being constructed twice; and a request-scoped component reading
//! native request data (a header and a cookie) via `RequestMeta`, reused across two
//! injections within one request.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::StatusCode;
use upwell::axum::axum::http::header::AUTHORIZATION;
use upwell::axum::axum::middleware::Next;
use upwell::axum::axum::response::Response;
use upwell::axum::axum::{Router, middleware as axum_middleware};
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{AxumMiddleware, RequestMeta, ScopeHandle};
use upwell::config::Toml;
use upwell::{ConfigManager, component, methods};

/// Reads a JSON response body into the given type.
async fn json_body<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// Protocol configuration is auto-bound and actively enforces server-wide limits.
// ---------------------------------------------------------------------------

#[controller(path = "/configured")]
struct ConfiguredController {}

#[handlers]
impl ConfiguredController {
    #[post("/echo")]
    async fn echo(&self, _body: upwell::axum::bytes::Bytes) {}

    #[get("/slow")]
    async fn slow(&self) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn axum_config_is_automatic_and_enforces_body_and_request_limits() {
    let config = ConfigManager::<Toml>::from_str(
        r#"
            [axum]
            bind = "127.0.0.1"
            port = 4321
            max_request_body_bytes = 4
            request_timeout_ms = 5
            graceful_shutdown_timeout_ms = 25
        "#,
    )
    .expect("parse axum config");
    let app = upwell::App::<upwell::axum::Axum>::builder("configured-axum")
        .auto_discover()
        .config_source(config)
        .build()
        .await
        .expect("app builds with the plugin-owned binding");

    assert_eq!(app.protocol().configured_addr().port(), 4321);
    assert_eq!(app.protocol().config().max_request_body_bytes, 4);

    let router: Router = app.protocol().router().clone();
    let too_large = router
        .clone()
        .oneshot(
            RawRequest::builder()
                .method("POST")
                .uri("/configured/echo")
                .body(Body::from("12345"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let timed_out = router
        .oneshot(
            RawRequest::builder()
                .uri("/configured/slow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(timed_out.status(), StatusCode::REQUEST_TIMEOUT);
}

// ---------------------------------------------------------------------------
// Scenario 1: a raw `from_fn` layer plus global/controller/path `AxumMiddleware` all run,
// in the documented order: scope-open (implicit) -> global -> controller -> path -> handler.
// ---------------------------------------------------------------------------

/// Records middleware call order, shared (via its internal `Arc`) across every DI middleware
/// and the introspection route in one app instance.
#[component(by_value)]
#[derive(Clone)]
struct CallLog {
    #[default]
    entries: Arc<Mutex<Vec<String>>>,
}

impl CallLog {
    fn record(&self, tag: &str) {
        self.entries.lock().unwrap().push(tag.to_string());
    }

    fn snapshot(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }
}

#[component]
struct GlobalMw {
    log: CallLog,
}

impl AxumMiddleware for GlobalMw {
    async fn handle(&self, req: RawRequest, next: Next) -> Response {
        self.log.record("global");

        next.run(req).await
    }
}

#[component]
struct ControllerMw {
    log: CallLog,
}

impl AxumMiddleware for ControllerMw {
    async fn handle(&self, req: RawRequest, next: Next) -> Response {
        self.log.record("controller");

        next.run(req).await
    }
}

#[component]
struct PathMw {
    log: CallLog,
}

impl AxumMiddleware for PathMw {
    async fn handle(&self, req: RawRequest, next: Next) -> Response {
        self.log.record("path");

        next.run(req).await
    }
}

#[controller(path = "/order", middleware = [ControllerMw])]
struct OrderController {
    log: CallLog,
}

#[handlers]
impl OrderController {
    /// Exercises the full middleware stack, then reports the order recorded so far: by the
    /// time this handler runs, every outer tier (raw, global, controller, path) has already
    /// fired, so one request is enough — no separate introspection call needed.
    #[get("/hit", middleware = [PathMw])]
    async fn hit(&self) -> Json<Vec<String>> {
        Json(self.log.snapshot())
    }
}

#[tokio::test]
async fn raw_layer_and_global_controller_path_middleware_run_in_order() {
    // A plain `axum::middleware::from_fn` closure — standard ecosystem middleware, not our
    // `AxumMiddleware` trait — reaching into the request's DI scope the same way `Inject` does,
    // to prove raw middleware isn't a second-class citizen relative to the DI-backed kind.
    let raw_layer = axum_middleware::from_fn(|req: RawRequest, next: Next| async move {
        if let Some(scope) = req.extensions().get::<ScopeHandle>().cloned()
            && let Ok(log) = scope.0.extract::<CallLog>().await
        {
            log.record("raw");
        }

        next.run(req).await
    });

    let app = upwell::App::<upwell::axum::Axum>::builder("test-order")
        .auto_discover()
        .layer(raw_layer)
        .middleware::<GlobalMw>()
        .build()
        .await
        .expect("app builds");

    let router: Router = app.protocol().router().clone();

    let response = router
        .oneshot(
            RawRequest::builder()
                .uri("/order/hit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let order: Vec<String> = json_body(response).await;
    assert_eq!(order, vec!["raw", "global", "controller", "path"]);
}

// ---------------------------------------------------------------------------
// Scenario 2: the same middleware type, registered both globally and on a controller, is one
// shared DI singleton — its state reflects both attach points firing, not two separate
// instances.
// ---------------------------------------------------------------------------

#[component]
struct SharedMw {
    #[default]
    calls: AtomicUsize,
}

impl AxumMiddleware for SharedMw {
    async fn handle(&self, req: RawRequest, next: Next) -> Response {
        self.calls.fetch_add(1, Ordering::SeqCst);

        next.run(req).await
    }
}

#[controller(path = "/shared", middleware = [SharedMw])]
struct SharedController {
    shared_mw: Arc<SharedMw>,
}

#[handlers]
impl SharedController {
    /// Registered at both the global tier and this controller: by the time this handler runs,
    /// one shared instance means both attach points have fired, so the count is 2 — a fresh
    /// instance per attach point would leave this controller-injected copy at 0 or 1.
    #[get("/hit")]
    async fn hit(&self) -> Json<usize> {
        Json(self.shared_mw.calls.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn same_middleware_type_shares_one_instance_across_attach_points() {
    let app = upwell::App::<upwell::axum::Axum>::builder("test-shared")
        .auto_discover()
        .middleware::<SharedMw>()
        .build()
        .await
        .expect("app builds");

    let router: Router = app.protocol().router().clone();

    let response = router
        .oneshot(
            RawRequest::builder()
                .uri("/shared/hit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let calls: usize = json_body(response).await;
    assert_eq!(calls, 2);
}

// ---------------------------------------------------------------------------
// Scenario 3: a request-scoped component built on `RequestMeta` reads a header and a cookie,
// and is reused (not re-fetched) across multiple injections within one request.
// ---------------------------------------------------------------------------

#[component(scope = HttpRequest)]
struct AuthUser {
    #[default]
    token: Option<String>,
    #[default]
    cookie: Option<String>,
}

#[methods]
impl AuthUser {
    #[init]
    async fn init(meta: RequestMeta) -> Self {
        Self {
            token: meta
                .headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            cookie: meta.cookies.get("session_id").cloned(),
        }
    }
}

#[dto]
struct WhoAmI {
    token: Option<String>,
    cookie: Option<String>,
    same_instance: bool,
}

#[controller(path = "/auth")]
struct AuthController;

#[handlers]
impl AuthController {
    #[get("/whoami")]
    async fn whoami(
        &self,
        Inject(a): Inject<Arc<AuthUser>>,
        Inject(b): Inject<Arc<AuthUser>>,
    ) -> Json<WhoAmI> {
        Json(WhoAmI {
            token: a.token.clone(),
            cookie: a.cookie.clone(),
            same_instance: Arc::ptr_eq(&a, &b),
        })
    }
}

#[tokio::test]
async fn request_scoped_component_reads_request_meta_and_is_shared() {
    let app = upwell::App::<upwell::axum::Axum>::builder("test-auth")
        .auto_discover()
        .build()
        .await
        .expect("app builds");

    let router: Router = app.protocol().router().clone();

    let response = router
        .oneshot(
            RawRequest::builder()
                .uri("/auth/whoami")
                .header("authorization", "Bearer abc123")
                .header("cookie", "session_id=sess-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let who: WhoAmI = json_body(response).await;
    assert_eq!(who.token.as_deref(), Some("Bearer abc123"));
    assert_eq!(who.cookie.as_deref(), Some("sess-xyz"));
    assert!(
        who.same_instance,
        "AuthUser should be fetched once and shared within the request"
    );
}
