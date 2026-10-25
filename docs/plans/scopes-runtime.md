# Scopes Runtime

Status: decided (2026-10-24). Open questions are settled when their tasks are implemented.
Umbrella for: #343 (per-call scope cost), #118 (invocation scope propagation), #117
(protocol-controlled scope lifecycle and seeds), #119 (factory-level and custom scopes)

## Goal

Make scopes fast, propagated and easy to author. A user can declare a scope for their own unit of
work, such as one customer sync, and open many instances of it, in sequence or in parallel. Each
instance has its own parameters, components and logs, and nothing leaks between instances.

Every scope entry is planned once per runtime generation (#343). Hot paths execute precomputed
steps; they never search or select providers.

## Decisions

### Access is controlled DI only

- **Public:** opening and entering scopes, propagating them into spawned work, and declared access:
  injected parameters, declared `Current<T>` and pinned `Scoped<S, T>` dependencies. Declared edges
  are visible to validation, tooling and entry planning.
- **Not public:** dynamic extraction from the active container, such as `upwell::current::<T>()`.
  It escapes the graph and defeats entry planning.
- **Framework-internal:** dynamic access to the active context lives in `upwell-di` as
  `pub(crate)`. Other framework crates get narrow, purpose-built `#[doc(hidden)]` hooks in a
  semver-exempt module ("route this event to the active log sink", "establish the context around
  this dispatch future"), never a general typed lookup.

### Spawned work holds its scope strongly

`upwell::spawn` and the other propagation primitives (#132, #267) carry the context and keep the
scope instance alive until the spawned work finishes. A plain `tokio::spawn` or
`std::thread::spawn` carries nothing.

### One context per task, holding the whole chain

The task-local context holds the active scope chain (innermost instance up to the root), the
runtime generation it pinned, and invocation metadata. `Current<T>` resolves from the innermost
scope that can own `T`; `Scoped<S, T>` resolves only from scope `S`. Neither caches across
invocations. A missing context is a typed error, never a panic.

## Design

### Scope identity moves onto the type

Before #404 a scope was a `&'static dyn Scope` value: `ScopeBoundary::new` took `&'static T` and
descriptors stored `scope: &'static dyn Scope`. A scope type with parameter fields cannot be a
static, so identity moved onto the type and parameters become per-instance values. This was a
breaking change to `ComponentDescriptor` and the topology types, landed first because placements,
layouts and seeds (#345, #347) are designed against it.

Implemented in #404 (owner, 2026-10-25):

- `StaticScope` is renamed to `Scope`, and the old object-safe `Scope` trait is removed. A scope
  type declares `ID`, `RANK`, `NAME` and optionally `IS_TRANSIENT`.
- Descriptors, containers and topologies carry a `Copy` `ScopeKey` (`S::KEY`, or
  `ScopeKey::of::<S>()`), compared and hashed by ID.
- `ScopeBoundary::new::<S>(&[ScopeParent::Root, ScopeParent::of::<Connection>()])` declares every
  opening parent. `PreparedScopeTopology` precomputes each boundary's chains; a dependency is
  reachable only when its scope is on every chain of the consumer (`common_ancestors`).
- Scope plans still order one component list per scope, over the common ancestors. Per-chain
  placements come with #345.


### Scope values: zero-sized or parameterised

A scope type is either zero-sized (no params, like `Request`) or a struct whose fields are its
params. Both use the same mechanism, and the zero-sized case costs nothing.

```rust
#[scope(parents = [Connection])]
pub struct Request;

#[scope(parents = [Root])]
pub struct CustomerSync {
    customer: CustomerId,
    credentials: Credentials,
}
```

The active scope's value is reachable two ways, both public:

- **Through DI:** components in the scope inject `Arc<CustomerSync>`.
- **Through the task-local context:** `#[scope]` generates `S::current() -> Result<Arc<S>, NoActiveScope>`.
  For a zero-sized scope it answers "am I inside one?", also offered as `S::is_active()`.

This refines the access rule. Reading the active scope's own value is allowed publicly, because it
builds nothing, is typed and closed, and each instance carries its own value. Resolving components
from the active container without a declared dependency stays non-public.

- Params are stored once per instance and shared by `current()` and injection, matching #344's
  single allocation per instance.
- Params are immutable, because one instance may run work in several tasks at once. Mutable
  per-instance state belongs in a scoped component.
- A `current()` read is not a dependency edge. It cannot build anything, so planning loses
  nothing, and tooling shows each scope's parameter fields from `#[scope]`.
- `CustomerSync { .. }.enter(future)` runs a future inside just the params context, without an
  app, so code that calls `current()` can be unit-tested.
- The name "scope handle" is avoided while axum's `ScopeHandle` (an `Arc<ScopeContainer>` in
  request extensions) exists; the value is plain `Arc<S>`.

### Scope parameters, seeds and options

Opening a scope takes three kinds of input, kept distinct:

| Input | What it is | Injectable | In logs and tooling |
|---|---|---|---|
| **Params** | The scope instance's identity and data, declared as fields on the scope type | Yes, as the scope type itself | Only fields marked loggable |
| **Seeds** | Pre-built component instances that fill declared seed slots | Yes, as their own types | No |
| **Options** | How this instance runs: log capture, eager construction | No | Yes |

```rust
/// One customer's synchronisation run.
#[scope(parents = [Root])]
pub struct CustomerSync {
    #[log]
    customer: CustomerId,
    credentials: Credentials,
}

#[component(scope = CustomerSync)]
struct CustomerClient {
    sync: Arc<CustomerSync>,
}

#[job(every = "1h")]
async fn sync_all(customers: Arc<CustomerRepo>, sync: ScopeOpener<CustomerSync>) -> Result<()> {
    let runs = customers.list().await?.into_iter().map(|customer| {
        sync.builder(CustomerSync::from(customer))
            .capture_logs()
            .run(|client: Arc<CustomerClient>| async move { client.sync().await })
    });

    futures::future::try_join_all(runs).await?;

    Ok(())
}
```

- The scope type is its own parameter set. Opening a scope takes a value of that type, and
  components in the scope inject it like any other dependency, so params need no separate seed.
- `ScopeOpener<S>` is the injectable form of the prepared opener (#346). It grants permission to
  open `S`, not to look anything up in it.
- `builder(params)` returns a scope builder: `.seed(value)` for declared seed slots, options such as
  `.capture_logs()`, then one of:
  - `.run(closure)`: a planned entry whose closure parameters declare its dependencies. The plan
    is cached per closure type.
  - `.enter(future)`: propagation only. The future runs inside the scope and reaches its values
    through declared `Current<T>` dependencies.

### Request data in protocol scopes

Protocol scopes keep request data in three tiers, so scope params stay cheap and non-secret:

| Tier | What | Where |
|---|---|---|
| Identity params | Request ID, method, matched route or RPC method name, peer address | Fields on the protocol's scope type, e.g. `Request { id, method, route }`, `Call { id, method }`. They label the scope's log span. |
| Request-bound data | Headers, cookies, query | `RequestMeta`, a lazy seed built only when declared (#347). |
| The request itself | Body, raw request head | Stays with the protocol's dispatch and its native extractors. |

- Full request data is not stored in the scope. Copying the head on every request is the cost #343
  removes, it would duplicate native extractors, and it would expose credentials to every reader
  of the task-local context.
- Request-bound sources are protocol-neutral: `RequestMeta` is built from a protocol-provided
  request-head source, not from axum's `Parts`. Any HTTP server can then back the same scope types
  (see #223), including a purpose-built Upwell server.
- The exact identity fields per protocol are decided in #353.

### Isolation between instances

- Every scope instance owns its own slots. Scoped components are never shared between instances.
- Captive-scope validation already stops a longer-lived component from holding a scoped instance.
  `Current<T>` is the only sanctioned way for a singleton to reach scoped values, and it never
  caches.
- Shared state a user deliberately puts in a singleton is outside the guarantee.

### Built-in log routing

Routing rides on `tracing`'s own span stack, not on the DI context, so it covers every `tracing`
event made inside a scope instance, including events from third-party crates.

**Mechanism**

1. Opening a scope instance enters an `upwell.scope` span carrying the scope name and its `#[log]`
   params, and instruments the instance's future with it. Propagation primitives instrument
   spawned work with the current span as well as the context.
2. The span's extensions hold the instance's log sink.
3. Upwell's `ScopeLogLayer` (a `tracing_subscriber::Layer`, installed by `init_tracing` or added to
   a user's own subscriber) handles each event by walking the event's span stack to the innermost
   span with a sink, and pushing an owned `LogRecord` into it.
4. Capture tees: every event still reaches the user's normal output.

**Capture is on by default, and costs nothing until someone subscribes**

`.without_log_capture()` opts out. With capture on, the sink starts records flowing on its first
subscriber; a scope nobody subscribes to costs one span-stack walk per event. Subscribing before
the run starts sees every record. Each subscriber has a bounded buffer and reports dropped records
as a `Lagged(n)` item rather than blocking the logging thread.

**Consuming logs as a stream**

```rust
let run = sync
    .builder(CustomerSync::from(customer))
    .run(|client: Arc<CustomerClient>| async move { client.sync().await });
let mut logs = run.logs();

let watcher = upwell::spawn(async move {
    while let Some(item) = logs.next().await {
        match item {
            ScopeLog::Record(record) if record.level <= Level::WARN => alert(&record).await,
            ScopeLog::Record(_) => {}
            ScopeLog::Lagged(dropped) => tracing::warn!(dropped, "scope log subscriber lagged"),
        }
    }
});

run.await?;
watcher.await?;
```

- `run.logs()` returns a `Stream` of owned items, so no lifetimes leak into user code. It ends
  when the scope instance closes, so collecting it gives the whole log at the end.
- Inside the scope, the same stream is available as a declared dependency: a component injecting
  `ScopeLogs` can forward or upload its own instance's records.
- A `LogRecord` holds the level, target, message, event fields, timestamp, and the scope's `#[log]`
  params.

**Guarantees**

- **Isolation:** an event only reaches the sink of the innermost capturing scope on its span stack,
  so parallel instances never see each other's records.
- **Completeness:** depends on propagation. Work spawned without the propagation primitives loses
  the span, so its events reach only the global output: uncaptured, never misrouted.
- **Levels:** capture sees only events the global filter enables. A lower capture level is an
  explicit per-scope option, because enabling a callsite for one layer enables it for every scope.

## Further decisions

- A scope may be opened from more than one parent. The user declares every opening parent on the
  scope (`#[scope(parents = [Root, Request])]`), so the planner knows the finite set of chains and
  plans each one.
- The end-of-scope hook is observer-only. It fires after the instance's work finishes and before
  its log sink closes, so its own events are still captured.

## Deferred to implementation

These are decided by the task that implements them, and recorded here when they are.


1. How a scope's params mark what may be logged: opt-in `#[log]`, or the config redaction model
   (`#[secret]` excludes, everything else logged).
2. Whether nested capturing scopes deliver to the innermost sink only, or also bubble to every
   capturing ancestor (a connection log containing its requests' records).
3. Buffer size and the default capture level.
