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
explicitly. The key defaults to `app/<function name in snake_case>`:

```rust
#[entry(scope = TenantJob)]
async fn reconcile(db: Arc<TenantDb>, limits: Cfg<Limits>) -> Result<()> { /* .. */ }

builder.entry::<Reconcile>()
```

Preparation rejects duplicate keys, scopes the protocol does not open, and targets that the
entry's scope cannot resolve. Reload candidates are checked the same way. The prepared app indexes
entries by dense `EntryId` in `PreparedApp::entries`, and `cargo upwell inspect` lists them as
`entry` resources.

### Application scopes

An application declares its own unit of work with `#[scope]`. The struct's fields are the
parameters of each instance, and its parent list generates `Scope`, one `OpensUnder<P>` per parent
and the `ScopeDeclaration` that adds the scope to the protocol's topology:

```rust
#[scope(parents = [Root])]
pub struct CustomerSync {
    customer: CustomerId,
}

#[component(scope = CustomerSync)]
struct CustomerClient {
    sync: Arc<CustomerSync>,
}

let synced = runtime
    .opener::<CustomerSync>()?
    .builder(CustomerSync { customer })
    .try_run(|client: Arc<CustomerClient>| async move { client.sync().await })
    .await?;
```

Discovery collects every declaration; `AppBuilder::scope::<S>()` registers one explicitly. Inside
an instance, components inject its value as `Arc<S>`. A unit scope stores nothing, so it has no
value to inject. `ScopeOpener<S>` is itself injectable as a parameter: of an `#[init]` constructor,
a job, an entry or `.run`/`.try_run` work. Injected, it holds the runtime weakly. `.run` (work
returning `()`) and `.try_run` (work returning a `Result`) plan their closure like
an entry whose targets are its parameters, once per parameter types and runtime generation, and
closes the instance when the work finishes.

A scope can also name `children`: scopes declared elsewhere, such as a protocol's request scope,
that may also open under it. `#[scope(parents = [Root], generations = durable,
children = [HttpRequest])] struct Session` lets an HTTP request open under a cached session, so
session-scoped components are built once per session. The child keeps its own parents, so its
components take the session's components optionally (`Option<Arc<T>>`). Code that picks the
parent per opening holds a `ParentScope<S>` (made from an `OpenScope<P>` where `S: OpensUnder<P>`)
and opens with `ScopeBuilder::under_parent`.

A value produced outside DI, such as an authenticated identity, is a `#[context(scopes = [..])]`:
factory-less and by value, it may be seeded into several scopes that share no hierarchy, each
instance through its own builder (`ScopeBuilder::seed`, or `.seeds(Seeds)` for seeds collected
ahead of the opening). The registry holds `SeedClaim`s saying who seeds what where: protocols
claim their own seeds and their endpoints' seeds, and `AppBuilder::provides_seed::<T, S>()`
claims a seed the application fills in a scope it opens itself. Preparation rejects a required
dependency on a context value whose scope no one claims (`SeedClaimError::UnprovidedSeed`) and a
claim for a scope with no slot of its type (`ClaimWithoutSlot`). Several openers may claim one
seed for one scope, since each fills its own instances.

### End-of-scope hook

`#[hook(ScopeEnded<Session>)] async fn ended(&self, end: ScopeEnd)` on a singleton observes every
`Session` instance ending, and `#[hook(AnyScopeEnded)]` observes every scope's (read `end.scope`). `ScopeEnd` carries the scope, how it ended (`Completed`, `Errored { error_type }` for
a `try_run` whose work returned `Err`, `Failed`, `Cancelled` for a run; `Released` when the last `OpenScope` or `EntryScope` handle, such as a protocol's
request scope, drops, `into_container` included) and the `#[log]` params. The hook is observer-only: it runs on a spawned
Tokio task after the instance's work and before its log closes, so the events it makes are
captured, and it cannot delay, veto or extend the instance. A failing or panicking hook is
traced on `upwell::scope`, never propagated. Graceful shutdown waits up to `END_HOOK_DRAIN` for
pending end hooks (`AppRuntime::drain_scope_ends` does it on demand).

### Scope log routing

The `upwell.scope` span names the instance an event was made in; the invocation context decides
whether that instance captures it, and `ScopeLogs::current()` resolves the same way. On a task or
thread where no invocation context is set at all, the span alone decides, so logs work without
the propagation machinery. Work captured outside every invocation (a `context::tokio` or
`context::rayon` job spawned outside one) never logs into a capture it happens to run inside.
Inside an invocation that runs in no scope instance (a job or a lifecycle hook), the span
decides as well. Inside one that does, an event reaches the span's instance only when that
instance is on the invocation's scope chain or is still opening under its innermost instance, as
its constructors do; otherwise it reaches the invocation's innermost capturing instance. Work
carried with the propagation primitives therefore logs to the invocation it was captured in,
even when it is called inside a child instance that opened under that invocation. One cost: an
`OpenScope` span entered by hand inside an invocation that does not run in it routes to that
invocation, not to the hand-entered instance, even when the instance opened under it.

`ScopeLogs::current()` uses that same resolution, so it reports where events made at that point
are delivered: `is_capturing()` is true when they are captured, including by an enclosing
capturing instance that a non-capturing one passes its events to, and it needs a
`ScopeLogLayer` whose filter admits the routing span, as capture does: an explicit
`upwell::scope=off` disables capture, and what `ScopeLogs` reports, on every path. It no longer reads a task-local of its own.

### Scope log filtering

With `tracing-subscriber` enabled, `init_tracing` uses the configured `RUST_LOG` or logging
filter for captured events. It additionally supplies an error-level fallback for the
`upwell.scope` routing span (target `upwell::scope`), so a filter such as `myapp=info` can
still attach a scope sink. This fallback does not enable additional captured events. An explicit
`upwell::scope=off` takes precedence over the static fallback; repeated and more specific
directives use `tracing-subscriber`'s `EnvFilter` precedence, and dynamic selectors can enable a
span independently.

Span- and field-specific selectors follow `EnvFilter`'s native behavior, not hard-exclusion
semantics. For example, `upwell::scope[upwell.scope]=off` does not override the static routing
fallback. Applications composing `ScopeLogLayer` into their own subscriber must configure its
filter to admit the routing span themselves.

## Internal role

The protocol crates (`upwell-rpc`, `upwell-axum`) build directly on this crate: each provides definition, prepared, and runtime states plus [`Serve`] implementations, and drives requests through the [`AppRuntime`] handle. The `upwell` facade re-exports the whole surface, and named `app!` declarations (in `upwell-macros`) generate lifecycle hosts whose configured builders target the [`AppBuilder`] here.

## Feature flags

| Feature | Effect |
|---|---|
| `cli` *(default)* | generated application bootstrap, Clap command dispatch, and lifecycle-aware command contexts |
| `tooling` | read-only projection of prepared application plans into the protocol-neutral tooling schema |
| `yaml` | forward YAML config support (`upwell-config/yaml`) |
| `watch` | forward config file watching/reload (`upwell-config/watch`) |
| `tracing-subscriber` | pull in `tracing-subscriber` for the `init_tracing` helper and the `ScopeLogLayer` scope log capture |
| `di-check` | compile-time DI graph validation (forwards to `upwell-di`/`upwell-config`) |
| `serde` | serialize redacted `RuntimeTransitionReport`s for tooling and telemetry (forwards to `upwell-di/serde`) |
