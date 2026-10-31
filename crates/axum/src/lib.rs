//! The Upwell axum/HTTP protocol, built on the protocol-agnostic `upwell-app` core.
//!
//! This crate provides the first-class [`Axum`] protocol definition and builds a real
//! [`axum::Router`] from `#[controller]`
//! components, bridges the framework's dependency injection into axum via the [`Inject`]
//! extractor (so route handlers mix native axum extractors with DI), and serves over HTTP.
//! Depend on it directly, or reach it through the `upwell` facade's `axum` feature.
//!
//! The DI bridge is deliberately thin and one-directional: nothing in `upwell-di` or
//! `upwell-core` knows axum exists. A per-request scope layer threads an
//! [`Arc<ScopeContainer>`](upwell_di::ScopeContainer) through the request extensions, and
//! [`Inject`] resolves components from it.

#[cfg(feature = "client")]
pub mod client;

/// The [`Dto`](dto::Dto) wire-data marker. Available on every target (the generated client and the
/// server both assert it), so it lives outside the server gate.
pub mod dto;

/// The wasm-safe, protocol-generic messaging wire contract (`MessagingProtocol`, `TopicCodec`,
/// `Topic`, `TopicParam`, …) shared by every WebSocket pub/sub protocol. Behind `ws` (not `stomp`)
/// and available on every target, so a non-STOMP protocol reuses the machinery and the wasm client
/// names it.
#[cfg(feature = "ws")]
pub mod messaging;

/// Stream framing. The markers (`Ndjson`/`RawStream`), the `StreamEncode` contract, and the
/// NDJSON encode/decode helpers are pure and compile everywhere (the generated streaming client
/// names them); the axum extractor/response impls inside are gated to non-wasm.
pub mod stream;

// Server-only modules: the DI bridge, controllers, protocol states, the serve loop, and the ws
// broker. None of
// these exist on a wasm client target, where only the generated HTTP client is compiled.
#[cfg(not(target_family = "wasm"))]
pub mod config;
#[cfg(not(target_family = "wasm"))]
pub mod controller;
#[cfg(not(target_family = "wasm"))]
pub mod error;
#[cfg(not(target_family = "wasm"))]
pub mod extract;
#[cfg(not(target_family = "wasm"))]
pub mod middleware;
/// OpenAPI document assembly: the link-time operation/schema slices the macros lower into, and
/// [`build_openapi`](openapi::build_openapi) which folds them into a `utoipa::openapi::OpenApi`.
/// Behind `openapi` (native server surface only); utoipa never reaches a wasm build.
#[cfg(all(feature = "openapi", not(target_family = "wasm")))]
pub mod openapi;
#[cfg(not(target_family = "wasm"))]
mod plugin;
#[cfg(not(target_family = "wasm"))]
pub mod protocol;
#[cfg(not(target_family = "wasm"))]
pub mod request_meta;
#[cfg(not(target_family = "wasm"))]
mod request_scope;
#[cfg(not(target_family = "wasm"))]
pub mod scope;
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub mod ws;

#[cfg(not(target_family = "wasm"))]
pub use config::{AXUM_CONFIG_PATH, AxumConfig};
#[cfg(all(feature = "openapi", not(target_family = "wasm")))]
pub use config::{AXUM_OPENAPI_CONFIG_PATH, OpenApiConfig, OpenApiUi};
/// The OpenAPI operation/schema slices and document assembler, re-exported at the crate root so
/// `#[dto]`/`#[handlers]` generated code registers into a stable path and the protocol folds them.
#[cfg(all(feature = "openapi", not(target_family = "wasm")))]
pub use openapi::{
    OPENAPI_OPERATIONS, OPENAPI_SCHEMAS, OperationEntry, SchemaEntry, build_openapi, join_base,
};

#[cfg(not(target_family = "wasm"))]
pub use controller::{
    CONTROLLERS, Controller, ControllerDescriptor, ControllerRoute, HttpInputDescriptor,
    HttpInputSource, HttpOutputDescriptor, HttpOutputShape, HttpPathParameterDescriptor,
    HttpResponseBodyDescriptor, HttpResponseDescriptor, HttpRouteDescriptor,
};
#[cfg(not(target_family = "wasm"))]
pub use error::{Error, Result};
/// The `utoipa` crate, re-exported so `#[dto]`/`#[handlers]` generated OpenAPI code names its
/// derive (`ToSchema`), attribute (`path`), and traits through a stable protocol path
/// (`::upwell_axum::utoipa` / `::upwell::axum::utoipa`) without the user crate depending on
/// `utoipa` directly. Native + `openapi` only.
#[cfg(all(feature = "openapi", not(target_family = "wasm")))]
#[doc(hidden)]
pub use utoipa;

#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use ws::{
    ControllerWsRoute, SOCKET_SEND_TIMEOUT, WS_CONTROLLERS, WebsocketController, WebsocketHandler,
    WebsocketProtocol, WebsocketUpgradeMeta, WsConnectionMeta, WsConnectionScope,
    WsConnectionSettings, WsControllerDescriptor, WsControllerRegistration, WsDispatchError,
    WsFuture, WsHandlerFn, WsIdle, WsMessageDescriptor, WsMessageMode, WsRespond, WsRoute,
    WsRouteDescriptor, WsShutdown,
};

/// The `PubSubProtocol` capability (server side): the seam a topic-bearing protocol implements so
/// the neutral registry/bus fan out for it. [`MessageReply`] is its request/response companion — a
/// protocol that routes a `#[message]` handler's non-unit return back to the requester. Behind `ws`
/// so a non-STOMP protocol implements them without enabling `stomp`.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use ws::{MessageReply, PubSubProtocol};

/// The protocol-generic pub/sub runtime (server side): the neutral subscription registry, the shared
/// topic bus, and the injected publisher. Behind `ws` so a non-STOMP protocol reuses them.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use ws::pubsub::{
    ConnectionId, DEFAULT_PUBLISH_FANOUT, Publisher, SubscriptionRegistry, TopicBus,
    register_topic_bus, topic_bus_descriptor,
};

/// The protocol-generic messaging wire contract, re-exported at the crate root on every target — the
/// browser client's generated `#[topics]`/`#[message]` code names it through the protocol path. Behind
/// `ws` so a non-STOMP protocol reuses it without enabling `stomp`.
#[cfg(feature = "ws")]
pub use messaging::{MessagingClientProtocol, MessagingProtocol, Topic, TopicCodec, TopicParam};

/// Re-exported so `#[topics]`-generated `Topic::encode` impls name the codec error without a
/// separate `upwell-transport` dependency. Behind `ws` so a non-STOMP protocol's generated code
/// names it too.
#[cfg(feature = "ws")]
pub use upwell_transport::CodecError;

/// The `multipart/form-data` request extractor, re-exported from axum under the `multipart` feature
/// so a handler reaches it through the facade (`upwell::axum::Multipart`) — pairing with the
/// client-side [`client::Multipart`] body builder used to send the upload.
#[cfg(all(feature = "multipart", not(target_family = "wasm")))]
pub use axum::extract::Multipart;
#[cfg(not(target_family = "wasm"))]
pub use extract::{Inject, InjectRejection, ScopeHandle};
#[cfg(not(target_family = "wasm"))]
pub use middleware::AxumMiddleware;
#[cfg(not(target_family = "wasm"))]
pub use plugin::{Axum, AxumAppBuilder, AxumAppServe, PreparedAxum};
#[cfg(not(target_family = "wasm"))]
pub use protocol::AxumRuntime;
#[cfg(not(target_family = "wasm"))]
pub use request_meta::RequestMeta;
/// The `StreamBody` request extractor is an axum `FromRequest` — server-only.
#[cfg(not(target_family = "wasm"))]
pub use stream::StreamBody;
/// Pure stream framing, available on every target (the generated streaming client names these).
pub use stream::{Ndjson, RawStream, StreamEncode, chunk_u8};
/// The topic-set macro (`#[topics]`). Behind `ws` so a non-STOMP protocol declares a topic set with
/// `#[topics(protocol = P)]`. The protocol is explicit so neutral code generation never names a
/// concrete downstream protocol.
#[cfg(feature = "ws")]
pub use upwell_axum_macros::topics;
/// The axum controller macros (`#[controller]`, `#[handlers]`, the route attributes), owned by
/// this protocol crate. Their generated code roots protocol types at this crate
/// (`::upwell_axum::*`) by default, or at `::upwell::axum::*` under the `facade` feature —
/// so they work whether `upwell-axum` is used directly or through the `upwell` facade. The
/// core macros (`app!`, `#[component]`, …) come from `upwell` (the always-present core).
pub use upwell_axum_macros::{
    controller, delete, get, handlers, head, message, options, patch, post, put, route,
};

/// Re-exported so streaming-client codegen can project a concrete stream's item type
/// (`<S as Stream>::Item`) and name the item stream it returns. Referenced only by generated code.
#[doc(hidden)]
pub use futures::Stream as __Stream;

/// The [`Dto`](dto::Dto) wire-data marker, re-exported at the crate root so `#[dto]`-generated impls
/// and the handler assertions name it through a stable path on every target.
pub use dto::Dto;

/// The axum controller macros' companion `#[dto]` attribute: derives `serde` (+ `tsify::Tsify` on
/// wasm) and marks a type [`Dto`] so it may cross the HTTP wire.
pub use upwell_axum_macros::dto;

/// The `bytes` crate, re-exported so raw-stream client codegen names `bytes::Bytes` through a
/// stable path (`::upwell_axum::bytes`) without depending on axum's re-export — the client
/// body/stream path stays wasm-safe. Referenced by generated code and available on every target.
pub use bytes;

/// The per-message scope a `#[message]` ws handler resolves its `Inject<T>` parameters from,
/// named by generated code. Referenced only by generated code.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
#[doc(hidden)]
pub type __MessageScope = upwell_app::EntryScope<crate::scope::WebsocketMessage>;

/// The DI scope container, re-exported for downstream WebSocket protocol implementations.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use upwell_di::ScopeContainer;

/// DI construction primitives needed by downstream protocol implementations that seed message
/// scope metadata or implement injectable authentication adapters.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, FromContainer, Injectable, Provided, Wiring,
};

#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use upwell_core::{Scope, TypeDescriptor};

/// The DI error returned while resolving protocol-owned connection dependencies.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use upwell_di::Error as DiError;

/// The standard HTTP request scope marker.
#[cfg(not(target_family = "wasm"))]
pub use scope::{HttpRequest, HttpRequest as HttpRequestScope};

/// Standard WebSocket scope markers available to downstream protocol implementations.
#[cfg(all(feature = "ws", not(target_family = "wasm")))]
pub use scope::{
    WebsocketConnection, WebsocketConnection as WebsocketConnectionScope, WebsocketMessage,
    WebsocketMessage as WebsocketMessageScope,
};

/// The axum app type: an [`App`](upwell_app::App) specialized to [`Axum`].
/// `App::builder(name)` resolves through this alias without a turbofish.
#[cfg(not(target_family = "wasm"))]
pub type App = upwell_app::App<Axum>;

/// The axum app builder: [`AppBuilder`](upwell_app::AppBuilder) specialized to [`Axum`].
#[cfg(not(target_family = "wasm"))]
pub type AppBuilder = upwell_app::AppBuilder<Axum>;

// Re-export the agnostic app surface so a standalone `upwell-axum` user has one import.
#[cfg(not(target_family = "wasm"))]
pub use upwell_app::{
    AppRegistry, AppRuntime, LoggingConfig, Plugin, PreparedProtocol, ProtocolDefinition,
    ProtocolRuntime, Serve, ServerConfig, ShutdownHandle, ShutdownSignal,
};

/// Re-exported so macro-generated code can reach the `#[distributed_slice]` attribute for
/// the `CONTROLLERS` slice through a stable path.
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use linkme;

/// Re-exported so macro-generated code can reach `inventory` (the alternate registration backend)
/// through a stable path, mirroring the `linkme` re-export.
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use inventory;

/// Re-exported so `#[controller]`/`#[handlers]` generated code and users reach axum through a
/// stable path without a separate dependency.
#[cfg(not(target_family = "wasm"))]
pub use axum;

/// Re-exported (mirroring the RPC protocol's own `pub use tower;`) so a raw `tower::Layer`
/// registered via [`AxumAppBuilder::layer`] — or a test driving the router with
/// `tower::ServiceExt::oneshot` — needs no separate `tower` dependency.
#[cfg(not(target_family = "wasm"))]
pub use tower;

/// The `http` crate (verb, headers, request/response), re-exported at the crate root so a
/// dependant resolves `::upwell_axum::http` — the path the generated client builds its
/// `http::Request` against. Natively this rides on axum's re-export; on wasm (no axum) the `http`
/// crate is depended on directly via the `client` feature.
#[cfg(not(target_family = "wasm"))]
pub use axum::http;
#[cfg(all(target_family = "wasm", feature = "client"))]
pub use http;
