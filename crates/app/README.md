# upwell-app

> The Upwell protocol-agnostic application core: `App`/`AppBuilder`, protocol states, plugins, the DI runtime handle, scope planning, lifecycle, and builtins.

Part of the [Upwell](../../README.md) framework — the application core, sitting above the config/DI/hooks/dirs layers and below the protocol crates (`upwell-rpc`, `upwell-axum`).

## Role

This crate ties the DI engine, config, hooks, and dirs into a runnable [`App`] that is generic over its [`ProtocolDefinition`]. It owns the [`AppBuilder`], the agnostic [`AppRegistry`], scope planning, the lifecycle/serve envelope, the [`AppRuntime`] handle a protocol drives requests through, and the `ProtocolDefinition -> PreparedProtocol -> ProtocolRuntime` transition. It is *protocol-agnostic*: it knows nothing of RPC, HTTP, or any wire format. A protocol is a sibling crate that implements these traits over this foundation.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. You meet it through named `app!` declarations or direct [`AppBuilder`] assembly, the typed setup/prepare/build/serve lifecycle they produce, and builtin config types like [`ServerConfig`] and [`LoggingConfig`]. The selected definition, such as `Rpc` or `Axum`, is the protocol-specific choice.

```rust
use upwell::{daemon::prelude::*, prelude::*};

app! {
    app NotifyApplication {
        name: "notifyd",
        protocol: Rpc,
        serve(_context, app) {
            let transport = TcpTransport::bind("127.0.0.1:7000").await?;

            app.serve(transport).await
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), upwell::CliError> {
    NotifyApplication::run().await
}
```

The [`app!` Rustdoc](https://docs.rs/upwell/latest/upwell/macro.app.html) is the authoritative
declaration, generated API, CLI, plugin, tooling, feature, and error reference. The
[named application migration guide](../../docs/named-application-migration.md) focuses on replacing
the removed expression form and choosing between a named host and the direct builder.

### Entries

Every unit of work that runs inside a scope registers one `EntryDescriptor`: its stable
`EntryKey`, the scope it runs in, and the dependencies it injects. Code that opens a scope itself
registers through the builder, and plugins through `PluginContributions::entry`:

```rust
builder.scope_entry::<TenantJob, (Arc<TenantDb>, Cfg<Limits>)>(
    upwell::namespaced_id!(EntryKey, "app/tenant-reconcile"),
)
```

A free function declares an entry with `#[entry]`. Its parameters are the targets, and the macro
generates a marker type named after it that discovery collects and the builder can register
explicitly. The key defaults to `app/<function name>`:

```rust
#[entry(scope = TenantJob)]
async fn reconcile(db: Arc<TenantDb>, limits: Cfg<Limits>) -> Result<()> { /* .. */ }

builder.entry::<Reconcile>()
```

Preparation rejects duplicate keys, scopes the protocol does not open, and targets that the
entry's scope cannot resolve. Reload candidates are checked the same way. The prepared app indexes
entries by dense `EntryId` in `PreparedApp::entries`, and `cargo upwell inspect` lists them as
`entry` resources.

## Internal role

The protocol crates (`upwell-rpc`, `upwell-axum`) build directly on this crate: each provides definition, prepared, and runtime states plus [`Serve`] implementations, and drives requests through the [`AppRuntime`] handle. The `upwell` facade re-exports the whole surface, and named `app!` declarations (in `upwell-macros`) generate lifecycle hosts whose configured builders target the [`AppBuilder`] here.

## Feature flags

| Feature | Effect |
|---|---|
| `cli` *(default)* | generated application bootstrap, Clap command dispatch, and lifecycle-aware command contexts |
| `tooling` | read-only projection of prepared application plans into the protocol-neutral tooling schema |
| `yaml` | forward YAML config support (`upwell-config/yaml`) |
| `watch` | forward config file watching/reload (`upwell-config/watch`) |
| `tracing-subscriber` | pull in `tracing-subscriber` for the `init_tracing` helper |
| `di-check` | compile-time DI graph validation (forwards to `upwell-di`/`upwell-config`) |
| `serde` | serialize redacted `RuntimeTransitionReport`s for tooling and telemetry (forwards to `upwell-di/serde`) |
