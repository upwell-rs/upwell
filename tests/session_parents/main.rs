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
use upwell::axum::axum::Extension;
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
use upwell::context::{Current, Scoped};
use upwell::scope::Root;
use upwell::{
    AppBuilder, AppRuntime, Cfg, ConfigManager, OpenScope, ScopeOpener, component, config, methods,
    scope, when,
};
use upwell_config::ResolverChain;

mod ends;
mod generations;
mod routing;
mod validation;

/// The header naming the caller whose session a request runs under.
const USER: &str = "x-user";

/// How many [`SessionClient`]s every session built, across this binary.
static CLIENT_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// How many requests reached the [`Sessions`] parent middleware, across this binary.
static PARENT_CALLS: AtomicUsize = AtomicUsize::new(0);

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
    /// Drops the cache's handle to the session of `user`.
    fn forget(&self, user: &str) {
        self.open
            .lock()
            .expect("the session cache is healthy")
            .remove(user);
    }

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
        PARENT_CALLS.fetch_add(1, Ordering::SeqCst);

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

/// Whether [`Watcher`] is enabled.
#[config(path = "gate")]
#[derive(Deserialize)]
struct GateCfg {
    accessor: bool,
}

/// A request-scoped value only an accessor reads.
#[component(scope = HttpRequest)]
struct RequestValue {
    #[default]
    label: String,
}

#[methods]
impl RequestValue {
    #[init]
    fn new() -> Self {
        Self {
            label: String::from("request value"),
        }
    }
}

/// A singleton reading [`RequestValue`] through the invocation context, only while the gate
/// is open. Disabling it leaves no accessor on the request scope in the next generation.
#[component]
#[when(GateCfg.accessor)]
struct Watcher {
    value: Current<Arc<RequestValue>>,
}

/// A session component keeping the [`Watcher`] of the generation its session opened on.
#[component(scope = Session)]
struct SessionWatcher {
    watcher: Option<Arc<Watcher>>,
}

/// Routes building a session's [`SessionWatcher`] and reading through the watcher it kept.
#[controller(path = "/watch")]
struct WatchController;

#[handlers]
impl WatchController {
    #[get("/warm")]
    async fn warm(&self, Inject(watcher): Inject<Option<Arc<SessionWatcher>>>) -> String {
        match watcher.and_then(|watcher| watcher.watcher.clone()) {
            Some(_) => String::from("watching"),
            None => String::from("unwatched"),
        }
    }

    /// Reads through the session's watcher without injecting anything request-scoped, so only
    /// an opening at once lets its accessor find the request scope.
    #[get("/read")]
    async fn read(&self, Extension(parent): Extension<RequestParent>) -> String {
        let watcher = parent
            .container()
            .get::<SessionWatcher>()
            .and_then(|session| session.watcher.clone())
            .expect("the session kept its watcher");

        match watcher.value.get().await {
            Ok(value) => value.label.clone(),
            Err(error) => format!("failed: {error}"),
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
    write_config(dir, value, Gate::Open);
}

/// Whether [`GateCfg`] enables [`Watcher`].
#[derive(Clone, Copy)]
enum Gate {
    Open,
    Closed,
}

fn write_config(dir: &Path, value: u32, gate: Gate) {
    let accessor = matches!(gate, Gate::Open);

    fs::write(
        dir.join("application.toml"),
        format!("[stamp]\nvalue = {value}\n\n[gate]\naccessor = {accessor}\n"),
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

/// The status and body a GET to `uri` answers with, as `user` when one is given.
async fn send(router: &Router, uri: &str, user: Option<&'static str>) -> (StatusCode, String) {
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
    let status = response.status();
    let body = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads");

    (
        status,
        String::from_utf8(body.to_vec()).expect("the body is UTF-8"),
    )
}

/// The body of a successful GET to `uri`, as `user` when one is given.
async fn get(router: &Router, uri: &str, user: Option<&'static str>) -> String {
    let (status, body) = send(router, uri, user).await;

    assert_eq!(status, StatusCode::OK, "GET {uri} succeeds: {body}");

    body
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
