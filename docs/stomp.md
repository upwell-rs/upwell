# STOMP over WebSocket — status

STOMP 1.2 pub/sub over WebSocket, implemented by the `upwell-axum-stomp` crate as an ordinary
downstream consumer of `upwell-axum`'s public protocol API. It is re-exported through the
`upwell` facade's `stomp` feature. This document tracks what v1 ships and what is deliberately
deferred.

Custom codecs implement `upwell_axum::TopicCodec<Stomp>` directly. Rust coherence does not
permit a protocol crate to blanket-adapt every downstream codec trait implementation; the bundled
`JsonCodec` provides the default adapter.

## Shipped in v1

- **Protocol seam.** `Stomp` is a `WebsocketProtocol` on the shared ws layer; a broker fans
  server-pushed `MESSAGE` frames out across connections. `register_ws::<Stomp>(path)` /
  `register_ws_with::<Stomp>(path, StompConfig)` mount it.
- **Frames.** `CONNECT`/`STOMP` → `CONNECTED` (with version negotiation over `accept-version`),
  `SUBSCRIBE`/`UNSUBSCRIBE`, `SEND`, server-pushed `MESSAGE`, `DISCONNECT`, and `ERROR` on a
  protocol violation. A hostless `CONNECT` (e.g. from stomp.js) is tolerated.
- **CONNECT authentication.** `StompConfig::with_authenticator` validates standard
  `login`/`passcode` credentials, custom CONNECT headers or the upgrade request's head
  (`StompConnect::upgrade()`) before `CONNECTED`. The connection scope opens only after a
  successful authentication, seeded with its `StompPrincipal` and any app seeds the
  authenticator returns (`StompAuthenticated::new(principal).seed(identity)`), so every
  connection- and message-scoped component can inject them and none is built for a rejected
  connection. An authenticator therefore resolves its arguments from the application root
  (singletons and configuration): an argument only a scope provides fails the endpoint build
  (`StompBuildError::ScopedAuthenticatorDependency`), and one the root cannot resolve at
  `CONNECT` rejects the connection (`StompAuthenticationError::Unresolved`). Rejection returns
  `ERROR` and never registers the connection with the broker. An app seed that is a
  `#[context]` value, such as an `Identity` also seeded per request over REST, is claimed for
  the connection scope with `StompConfig::provides_seed::<Identity>()`, which needs an
  authenticator to produce it. Each endpoint declares the seeds it fills, and preparation
  rejects a required context value of the connection scope that any endpoint, STOMP or not,
  does not seed (`OpenerWithoutSeed`, naming that endpoint).
- **App handlers.** `#[controller(ws = Stomp)]` + `#[handlers(ws = Stomp)] #[message("/app/..")]`;
  handlers get frame headers (`Inject<StompHeaders>`), a session (`Inject<StompSession>`), and a
  typed publisher (`Inject<Publisher<T>>`), plus the usual request-scoped DI.
- **Typed topics.** `#[topics]` is the single source of truth. Static (`Chat(Msg)`) and
  **templated** (`Room { room: String, #[content] msg }` with `#[topic("/topic/room/{room}")]`)
  variants; named fields fill `{hole}`s via `TopicParam`. Generates `impl Topic` (server publish)
  and a `{Enum}Client<C>` with typed `subscribe_<variant>(..)` methods (client subscribe).
- **Typed client.** `StompClientTransport` is one connection, `Clone`-shared across the generated
  `{Controller}Client` (typed `send`s) and `{Topics}Client` (typed `subscribe`s). A `Subscription`
  is a `Stream` that `UNSUBSCRIBE`s on drop. `StompConnectOptions` carries credentials/custom
  headers, and explicit `disconnect()` closes the socket for every cloned client handle (last-handle
  drop remains a best-effort fallback).
- **Pluggable codec.** `StompCodec` (default `JsonCodec`), selected per surface with
  `#[topics(codec = ..)]` and `#[handlers(ws = Stomp, codec = ..)]`. The SEND path is codec-agnostic
  and symmetric (client encode = server decode).
- **Integrations.** `TopicParam` has explicit std impls; the cross-cutting `uuid` feature adds
  `TopicParam for uuid::Uuid`. The facade fans an integration flag out only to enabled crates
  (`uuid = ["upwell-axum?/uuid"]`).

## Not yet implemented (deferred past v1)

Each has its seam reserved, so it can be added without reshaping what exists.

- **Client-requested RECEIPT.** The server emits a `RECEIPT` when a frame carries a `receipt`
  header, and the client's read loop has a receipt-demux table wired — but there is no public
  `send_receipted`-style API to request one. `StompSend` is fire-and-forget: it acknowledges the
  socket write, not a broker `RECEIPT`.
- **Heart-beating (negotiation + liveness).** The server can emit heart-beats
  (`StompConfig::server_heartbeat`, default off) and both sides consume an inbound heart-beat frame,
  but the `heart-beat` header is not negotiated for timing, the client sends none of its own, and
  neither side runs a liveness/timeout watchdog. `CONNECT` negotiation reads `accept-version` only.
- **ACK modes.** `SUBSCRIBE` is always `ack: auto`; `ACK`/`NACK` frames and the `client` /
  `client-individual` acknowledgement modes are not handled.
- **Transactions.** `BEGIN`/`COMMIT`/`ABORT` and transactional `SEND`/`ACK` are not handled.
- **Destination wildcards / patterns.** The broker matches destinations exactly (behind a
  `DestinationMatch` seam kept for future prefix/wildcard matching); pattern subscriptions such as
  `/topic/**` are not expanded.

## Notes

- The `/app` SEND payload and each topic's broadcast body are encoded independently — the
  `#[handlers(codec = ..)]` and `#[topics(codec = ..)]` codecs are chosen per surface, so a
  controller and its topics may use different wire formats.
- Framing uses the [`stomp-parser`](https://crates.io/crates/stomp-parser) crate (deps: `either`,
  `nom`, `paste` — no `openssl`/`rsa`).
- End-to-end tests: `examples/http/tests/stomp.rs` (typed client round trip, per-room templated
  subscription, custom-codec round trip) and `examples/http/src/stomp.rs` (chat controller).
