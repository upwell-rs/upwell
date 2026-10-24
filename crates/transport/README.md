# upwell-transport

> Wire transport and status-code types for the Upwell daemon framework.

Part of the [Upwell](../../README.md) framework — the transport substrate the RPC protocol and client build on.

## Role

This crate owns the [`Transport`]/[`Connection`]/[`Respond`] abstraction, the length-prefixed wire [`protocol`], the [`StatusCode`]/[`Flags`] status types, the [`Encodes`]/[`Decodes`] codec traits, and concrete transports (TCP, Unix sockets, in-memory). The daemon is generic over [`Transport`]: a transport yields [`Connection`]s, each of which yields `(IncomingCall, Responder)` pairs. Correlation ids ([`CallId`]) live entirely here — the daemon never sees them. The `Transport`/`Connection` traits stay available on wasm; only the socket/in-memory impls (which drive `tokio::net`/`mio`) are compiled out there.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. You touch it when choosing what to serve an RPC daemon over — [`TcpTransport`] or (on unix) [`UnixTransport`] — and, on a connection-scoped component, by injecting [`PeerInfo`] when the `di` feature is on.

```rust
use upwell::daemon::prelude::*;

// Serve over any `Transport` — TCP here, or a Unix socket on unix targets.
app.serve(TcpTransport::bind("127.0.0.1:7000").await?).await
```

## Internal role

`upwell-client` re-exports this crate's [`Encodes`]/[`Decodes`]/[`CodecError`] and builds its `ClientError` on this crate's [`Error`]. The native RPC protocol (`upwell-rpc`) drives requests over the [`Transport`]/[`Connection`] seam and carries this crate's packed [`StatusCode`] on its responses. [`PeerInfo`] becomes a DI injectable via the `di` feature so connection-scoped components can depend on it.

## Feature flags

| Feature | Effect |
|---|---|
| `di` | make [`PeerInfo`] an Upwell DI injectable (pulls in `upwell-di`); off by default so the transport stays standalone |
| `di-check` | additionally emit the compile-time `Provided<PeerInfo>` impl for di-check (implies `di`) |
