# upwell-axum

> The first-class axum/HTTP protocol for Upwell: controllers, DI route extractors, WebSockets/STOMP, multipart, and a generated typed client.

Part of the [Upwell](../../README.md) framework — the HTTP protocol layer over the protocol-agnostic `upwell-app` core.

## Role

`upwell-axum` provides the `Axum -> PreparedAxum -> AxumRuntime` protocol states and builds a real [`axum::Router`] from
`#[controller]` components and serves them over HTTP. It bridges the framework's dependency
injection into axum via the [`Inject`] extractor — a per-request scope layer opens the request's
`ScopeContainer` on first use and keeps it for the request, and `Inject<T>` resolves components from it, so
route handlers freely mix native axum extractors with DI. The bridge is deliberately thin and
one-directional: nothing in `upwell-di` or `upwell-core` knows axum exists.

Axum declares two distinct scope paths rather than treating every inbound operation as a generic
request: `root -> HttpRequest` for HTTP, and, with `ws`,
`root -> WebsocketConnection -> WebsocketMessage`. Components select the concrete marker they need,
for example `#[component(scope = HttpRequest)]` or
`#[component(scope = WebsocketMessage)]`. `RequestMeta` is seeded only into `HttpRequest`;
WebSocket connection components use `WebsocketUpgradeMeta` for the upgrade request and
`WsConnectionMeta` for negotiated connection state.

A request opens its `HttpRequest` scope the first time something injects from it, so a route that
injects nothing pays for no scope. An eager component (`#[component(scope = HttpRequest, eager)]`)
changes that: it runs for every routed request, after global middleware and before controller
and path middleware. So a request global middleware rejects usually runs no eager component (it
does if that middleware injected from the scope first), and a request no route matches (a 404)
runs none. RPC differs here: it opens a call's scope before its middleware runs.

`RequestMeta` copies the request's method, URI and headers, so it is captured only when something
reads it, once per request: when the scope opens if the route reads it (directly or through a
component it builds or reaches lazily), otherwise at the first `Inject` that reads it. A route
that never reads it copies no headers. Each reader of one request sees the same capture, so a
read the route declares sees the head as it was when the scope opened, while a read that only
middleware makes sees it as it was at that read. `ScopeHandle` is the exception: a handle can
resolve anything later, after axum has consumed the head, so handing one out (`ScopeHandle` as
an extractor or `ScopeHandle::of`) costs one header-map clone even if nothing reads
`RequestMeta`. Prefer `Inject<T>` where the type is known. `ScopeHandle` is not a request
extension: reach the scope through the extractor or `ScopeHandle::of`.

Beyond plain REST it owns the HTTP-side extras that mirror the RPC protocol for the web: WebSocket
controllers ([`WebsocketController`], `#[controller(ws = ..)]`), a STOMP 1.2 pub/sub [`Broker`] with
a typed `#[topics]` contract, the `multipart/form-data` extractor, NDJSON/raw stream framing, and a
transport-generic generated client (`crate::client`) that also compiles to wasm/TypeScript.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate under
`upwell::axum` — you rarely name it directly. Enable the `axum` feature; it pulls in the HTTP
protocol plus the controller macros. Declare `#[controller]`/`#[handlers]` with route attributes,
then build an [`App`] (an `App<Axum>`) and serve.

```rust
use upwell::axum::prelude::*;
use upwell::prelude::*;

#[dto]
pub struct Greeting {
    pub message: String,
}

#[controller(path = "/greet")]
pub struct GreetController;

#[handlers]
impl GreetController {
    /// `GET /greet/{who}` — mixes the axum `Path` extractor with DI-resolved state via `Inject`.
    #[get("/{who}")]
    async fn greet(&self, Path(who): Path<String>) -> Json<Greeting> {
        Json(Greeting { message: format!("Hello, {who}!") })
    }
}
```

The generated `GreetControllerClient` (under the `client` feature, with a `reqwest`/`hyper` backend)
issues typed calls from the same definition, and the crate compiles to `wasm32` as a browser client.

## Internal role

Native (server) builds depend on `upwell-app`, `upwell-core`, `upwell-di`,
`upwell-config`, and `upwell-hooks`, plus `axum`/`tower`/`tokio`. It re-exports the agnostic
app surface (`App`, `AppBuilder`, `ProtocolDefinition`, `Serve`, …) so a standalone `upwell-axum`
user has a single import. The macros come from the sibling `upwell-axum-macros` crate (re-exported
here: `controller`, `handlers`, the route attrs, `dto`, `topics`), and codecs build on
`upwell-transport`. The `upwell` facade re-exports everything under `upwell::axum`. Because
the generated client must build for wasm, the client/DTO/stream/STOMP-wire modules are wasm-safe,
while the server modules (controller, plugin, protocol, ws broker, extractors) are gated to non-wasm.

## Feature flags

| Feature | Effect |
|---|---|
| `client` | Compile the generated HTTP client SDK (envelopes + codec, transport-agnostic). Pick a backend to issue requests. |
| `reqwest` | reqwest client backend (rustls-tls, streaming; also the wasm `fetch` backend and browser `Connection`). Implies `client`. |
| `hyper` | hyper client backend. Implies `client`. |
| `tungstenite` | WebSocket/STOMP client transports (native + wasm). Implies `client` + `ws`. |
| `ws` | WebSocket controller support (`axum/ws`). |
| `stomp` | STOMP 1.2 pub/sub broker + `#[controller(ws = Stomp)]` handlers + typed client (`stomp-parser`). Implies `ws`. |
| `multipart` | The server-side `multipart/form-data` extractor (re-exported `axum::extract::Multipart`). |
| `uuid` | Pull in `uuid`; the `TopicParam for Uuid` impl activates under `all(stomp, uuid)`. |
| `wasm-ts` | Opt into the newer `tsify` `Ts<T>` wasm ABI for the browser client (needs unreleased `tsify`). |
| `yaml` / `watch` / `tracing-subscriber` | Forwarded config extras: YAML sources / reload on change / `init_tracing`. |
| `di-check` | Compile-time DI-graph validation (forwarded across app/di/config/macros). |
| `facade` | Set by the `upwell` facade: root the macros' generated protocol types at `::upwell::axum::*` instead of the standalone `::upwell_axum::*`. |
