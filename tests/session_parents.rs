#![cfg(feature = "axum")]

//! End-to-end tests for HTTP request scopes opened under an application scope the app picks
//! per request: `Singleton -> Session -> HttpRequest`.
//!
//! [`Session`] lists [`HttpRequest`] among its children, so a request may open under the root
//! or under a session. [`Sessions`], a parent middleware, opens one durable [`Session`] per
//! `x-user` header, caches it, and hands it to the request as a [`RequestParent`]; a request
//! without the header opens under the root. [`SessionClient`] counts how often a session builds
//! it, and [`Stamp`] names the runtime generation a reader resolved on.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tempfile::TempDir;
use upwell::axum::axum::Router;
use upwell::axum::axum::body::{self, Body};
use upwell::axum::axum::extract::Request as RawRequest;
use upwell::axum::axum::http::{HeaderValue, StatusCode};
use upwell::axum::axum::middleware::{self as axum_middleware, Next};
use upwell::axum::axum::response::Response;
use upwell::axum::prelude::*;
use upwell::axum::tower::ServiceExt;
use upwell::axum::{AxumMiddleware, RequestParent};
use upwell::config::Toml;
use upwell::context::Scoped;
use upwell::scope::Root;
use upwell::{
    AppBuilder, AppRuntime, Cfg, ConfigManager, OpenScope, ScopeOpener, component, config, methods,
    scope,
};
use upwell_config::ResolverChain;

/// The header naming the caller whose session a request runs under.
const USER: &str = "x-user";

/// How many [`SessionClient`]s every session built, across this binary.
static CLIENT_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// The user the last [`EagerProbe`] saw, or `root` without a session.
static EAGER_SAW: Mutex<Option<String>> = Mutex::new(None);

/// One user's session: a durable scope requests may open under.
#[scope(parents = [Root], generations = durable, children = [HttpRequest])]
pub struct Session {
    user: String,
}

/// The config each runtime generation's [`Stamp`] is built from.
#[config(path = "stamp")]
#[derive(Deserialize)]
struct StampCfg {
    value: u32,
}

/// A singleton holding the config value its generation was built from.
#[component]
struct Stamp {
    #[default]
    value: u32,
}

#[methods]
impl Stamp {
    #[init]
    fn new(cfg: Cfg<StampCfg>) -> Self {
        Self {
            value: cfg.get().value,
        }
    }
}

/// A per-user client, built once per session.
#[component(scope = Session)]
struct SessionClient {
    #[default]
    user: String,
}

#[methods]
impl SessionClient {
    #[init]
    fn new(session: Arc<Session>) -> Self {
        CLIENT_BUILDS.fetch_add(1, Ordering::SeqCst);

        Self {
            user: session.user.clone(),
        }
    }
}

/// A request-scoped view of the request's session, if it runs under one, and of the stamp of
/// its generation.
#[component(scope = HttpRequest)]
struct RequestView {
    client: Option<Arc<SessionClient>>,
    stamp: Arc<Stamp>,
}

impl RequestView {
    fn describe(&self) -> String {
        let user = self
            .client
            .as_ref()
            .map_or("root", |client| client.user.as_str());

        format!("{user}@{}", self.stamp.value)
    }
}

/// An eager request-scoped component recording the session it saw.
#[component(scope = HttpRequest, eager)]
struct EagerProbe {}

#[methods]
impl EagerProbe {
    #[init]
    fn new(client: Option<Arc<SessionClient>>) -> Self {
        let user = client.map_or_else(|| String::from("root"), |client| client.user.clone());

        *EAGER_SAW.lock().expect("the probe lock is healthy") = Some(user);

        Self {}
    }
}

/// A singleton reading the active session's client through the invocation context.
#[component]
struct SessionReader {
    client: Scoped<Session, Arc<SessionClient>>,
}

/// The parent middleware: one cached [`Session`] per user.
#[component]
struct Sessions {
    #[default]
    opener: Option<ScopeOpener<Session>>,
    #[default]
    open: Mutex<HashMap<String, OpenScope<Session>>>,
}

#[methods]
impl Sessions {
    #[init]
    fn new(opener: ScopeOpener<Session>) -> Self {
        Self {
            opener: Some(opener),
            open: Mutex::new(HashMap::new()),
        }
    }
}

impl Sessions {
    /// The cached session of `user`.
    fn get(&self, user: &str) -> Option<OpenScope<Session>> {
        self.open
            .lock()
            .expect("the session cache is healthy")
            .get(user)
            .cloned()
    }

    /// The session of `user`, opened on its first request.
    async fn session(&self, user: &str) -> OpenScope<Session> {
        if let Some(session) = self.get(user) {
            return session;
        }

        let opener = self.opener.as_ref().expect("an injected session opener");
        let session = opener
            .builder(Session {
                user: user.to_owned(),
            })
            .open_on_demand()
            .await
            .expect("the session opens");

        self.open
            .lock()
            .expect("the session cache is healthy")
            .entry(user.to_owned())
            .or_insert(session)
            .clone()
    }
}

impl AxumMiddleware for Sessions {
    async fn handle(&self, mut request: RawRequest, next: Next) -> Response {
        let user = request
            .headers()
            .get(USER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        if let Some(user) = user {
            let session = self.session(&user).await;

            request
                .extensions_mut()
                .insert(RequestParent::new(&session));
        }

        next.run(request).await
    }
}

/// Routes reporting the session and generation a request saw.
#[controller(path = "/session")]
struct SessionController;

#[handlers]
impl SessionController {
    #[get("/view")]
    async fn view(&self, Inject(view): Inject<Arc<RequestView>>) -> String {
        view.describe()
    }

    #[get("/client")]
    async fn client(&self, Inject(client): Inject<Option<Arc<SessionClient>>>) -> String {
        client.map_or_else(|| String::from("root"), |client| client.user.clone())
    }
}

/// A route reading the session through [`SessionReader`], registered only by the tests that
/// register the reader.
#[controller(path = "/scoped")]
struct ScopedController;

#[handlers]
impl ScopedController {
    #[get("/read")]
    async fn read(&self, Inject(reader): Inject<Arc<SessionReader>>) -> String {
        match reader.client.get().await {
            Ok(client) => client.user.clone(),
            Err(_) => String::from("outside"),
        }
    }
}

/// A config directory holding `value`.
fn config_dir(value: u32) -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("upwell-session-parents-")
        .tempdir()
        .expect("create temp config dir");

    write_value(dir.path(), value);

    dir
}

fn write_value(dir: &Path, value: u32) {
    fs::write(
        dir.join("application.toml"),
        format!("[stamp]\nvalue = {value}\n"),
    )
    .expect("write config");
}

/// The app serving the session routes from the config in `dir`, without a parent layer.
fn app(dir: &Path) -> AppBuilder<Axum> {
    let config = ConfigManager::<Toml>::load_in_with_resolvers(dir, &[], ResolverChain::empty())
        .expect("load config");

    upwell::App::<Axum>::builder("test-session-parents")
        .config_source(config)
        .config::<StampCfg>("stamp")
        .scope::<Session>()
        .controller::<SessionController>()
        .component::<Stamp>()
        .component::<SessionClient>()
        .component::<RequestView>()
}

/// [`app`] with [`Sessions`] as its parent middleware.
fn with_sessions(dir: &Path) -> AppBuilder<Axum> {
    app(dir).parent_middleware::<Sessions>()
}

async fn build(builder: AppBuilder<Axum>) -> (Router, AppRuntime) {
    let app = builder.build().await.expect("app builds");
    let runtime = app.runtime().clone();

    (app.protocol().router().clone(), runtime)
}

/// The body a GET to `uri` answers with, as `user` when one is given.
async fn get(router: &Router, uri: &str, user: Option<&'static str>) -> String {
    let mut request = RawRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request is valid");

    if let Some(user) = user {
        request
            .headers_mut()
            .insert(USER, HeaderValue::from_static(user));
    }

    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");

    assert_eq!(response.status(), StatusCode::OK, "GET {uri} succeeds");

    let body = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads");

    String::from_utf8(body.to_vec()).expect("the body is UTF-8")
}

/// Middleware putting a [`RequestParent`] into every request, registered as plain global
/// middleware rather than a parent layer.
async fn stray_parent(mut request: RawRequest, next: Next) -> Response {
    let sessions = request
        .extensions()
        .get::<Arc<Sessions>>()
        .map(Arc::clone)
        .expect("the test installs the session cache");
    let session = sessions.session("stray").await;

    request
        .extensions_mut()
        .insert(RequestParent::new(&session));

    next.run(request).await
}

#[tokio::test]
async fn requests_open_under_their_session_or_the_root_in_one_app() {
    let dir = config_dir(1);
    let (router, _runtime) = build(with_sessions(dir.path())).await;

    assert_eq!(get(&router, "/session/view", Some("ada")).await, "ada@1");
    assert_eq!(get(&router, "/session/view", None).await, "root@1");
    assert_eq!(get(&router, "/session/view", Some("bob")).await, "bob@1");
    assert_eq!(get(&router, "/session/client", Some("ada")).await, "ada");
    assert_eq!(get(&router, "/session/client", None).await, "root");
}

#[tokio::test]
async fn a_session_builds_its_components_once_across_its_requests() {
    let dir = config_dir(1);
    let (router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");

    for _ in 0..3 {
        assert_eq!(get(&router, "/session/view", Some("cy")).await, "cy@1");
    }

    assert_eq!(get(&router, "/session/client", Some("cy")).await, "cy");
    assert_eq!(get(&router, "/session/view", Some("dan")).await, "dan@1");

    let client = sessions
        .get("cy")
        .expect("the session is cached")
        .container()
        .get::<SessionClient>()
        .expect("the session holds its client");

    assert_eq!(client.user, "cy");
    assert_eq!(
        CLIENT_BUILDS.load(Ordering::SeqCst),
        2,
        "each session built its client once, whatever its requests injected"
    );
}

#[tokio::test]
async fn eager_request_components_and_scoped_accessors_see_the_session() {
    let dir = config_dir(1);
    let (router, _runtime) = build(
        with_sessions(dir.path())
            .controller::<ScopedController>()
            .component::<EagerProbe>()
            .component::<SessionReader>(),
    )
    .await;

    assert_eq!(get(&router, "/scoped/read", Some("dee")).await, "dee");
    assert_eq!(
        EAGER_SAW
            .lock()
            .expect("the probe lock is healthy")
            .as_deref(),
        Some("dee"),
        "the eager opening ran under the session"
    );
    assert_eq!(get(&router, "/scoped/read", None).await, "outside");
    assert_eq!(
        EAGER_SAW
            .lock()
            .expect("the probe lock is healthy")
            .as_deref(),
        Some("root")
    );
}

#[tokio::test]
async fn a_session_keeps_its_generation_after_a_reload() {
    let dir = config_dir(1);
    let (router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");

    assert_eq!(get(&router, "/session/view", Some("eve")).await, "eve@1");

    write_value(dir.path(), 2);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(
        report.is_committed(),
        "a changed value commits a generation"
    );

    let session = sessions.get("eve").expect("the session is cached");

    assert_eq!(
        runtime.generations_behind(session.container()),
        Some(1),
        "the session reports how far behind it is"
    );
    assert_eq!(
        get(&router, "/session/view", Some("eve")).await,
        "eve@1",
        "a request under the session runs on the session's generation"
    );
    assert_eq!(
        get(&router, "/session/view", None).await,
        "root@2",
        "a request under the root runs on the current generation"
    );
    assert_eq!(
        get(&router, "/session/view", Some("fay")).await,
        "fay@2",
        "a session opened after the reload runs on the current generation"
    );
}

#[tokio::test]
async fn without_a_parent_layer_a_request_parent_is_ignored() {
    let dir = config_dir(1);
    let (sessions_router, runtime) = build(with_sessions(dir.path())).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");
    let (router, _runtime) = build(
        app(dir.path())
            .layer(upwell::axum::axum::Extension(sessions))
            .layer(axum_middleware::from_fn(stray_parent)),
    )
    .await;

    assert_eq!(get(&sessions_router, "/session/view", None).await, "root@1");
    assert_eq!(
        get(&router, "/session/view", Some("gus")).await,
        "root@1",
        "the request opens under the root, as without the feature"
    );
}
