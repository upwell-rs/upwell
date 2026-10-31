# Scopes Runtime

Status: decided (2026-10-24, generation policy and the single opening API added 2026-10-28). Open
questions are settled when their tasks are implemented.
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
  opening parent. `PreparedScopeTopology` precomputes each boundary's common ancestors (the
  intersection over its parents, never an enumeration of chains, which can grow exponentially);
  a dependency is reachable only when its scope is on every chain of the consumer.
- Scope plans still order one component list per scope, over the common ancestors. Per-chain
  placements come with #345.

### Slot layouts and planned dependencies

Implemented in the first slice of #345 (owner, 2026-10-29):

- Each protocol-owned scope gets a `SlotLayout` when a generation is prepared: its components in one
  topological order, seeds first, plus a type index and a trait index built once. Layouts are
  planned parents first, so a layout can point into the layouts of the scopes open around it.
  Transitions plan new layouts with the candidate graph.
- Every eager edge of every factory is resolved once to a `DepRef`: a root singleton, a slot of an
  ancestor's layout, a slot of the scope's own layout, a transient with its own planned edges, every
  provider of a collection or keyed edge, or absent for an unsatisfied optional edge. Selection uses
  the same rules and the same reachability as validation, so a planned edge resolves to the target
  validation checked. Config, dynamic, lazy, deferred and fresh edges keep resolving through their
  own handles. They stay in the plan as unplanned entries, so every edge keeps its declared
  position.
- A scope opened by the runtime stores its instances in a dense slot store laid out by its layout,
  instead of per-instance `HashMap`s. Factories read their planned edges first, so no provider
  selection runs while a planned scope is built. A planned source that cannot answer, such as an
  ancestor opened without a layout, falls back to dynamic resolution.
- Construction fails with attributed errors (`MissingDependency`, `UnplacedDependency`) when an
  edge has no source on the chain. Captive and ambiguous edges are still rejected by validation
  first.
- While #136 and #140 are open, a placement is the component's declared scope.
- The pinned storage handle is decided: `Live<T>` becomes an enum internally (a fixed `Arc<T>` or the
  swappable `ArcSwap`), so slots in a pinned scope need no `ArcSwap` and `Dep<T>` keeps its type.
  Slots store `Injectable::Stored`, so layouts do not change when it does. The first #344 slice
  landed it for every scope below the root: only transitions republish `Live` cells, and they
  only republish root singletons, so a child scope's factories store fixed handles
  (`Injectable::into_fixed`, chosen by the construction context). A `Dep<T>` read from a fixed
  slot gets its own cell, and provider aliases keep the kind of the instance they alias. The root,
  transients and `Fresh` rebuilds keep swappable cells.
- Generated factories read their edges by declared position (`cx.dep(i)`, `cx.dep_all(i)`,
  `cx.dep_keyed(i)`), so a planned edge costs one index instead of a search by type. `#[component]`
  field injection emits these calls, and `#[init]`/`factory = path` factories pass each parameter's
  position through `FromContainer::from_container_at`. Hand-written `FromContainer` parameters keep
  the typed lookup by default. Debug builds panic when an edge is read as a different type from the
  one declared at its position; release builds fall back to the typed lookup.
- A factory may also carry a synchronous `construct_ready` path (#430), tried before the boxed async
  `construct`. It reads planned edges without awaiting (`cx.dep_ready(i)` and friends,
  `FromContainer::ready_at`) and returns `Ok(None)` when an edge needs a transient build or dynamic
  selection, so the async path runs instead. `#[component]` field injection and sync `#[init]`
  constructors emit one; async and hand-written factories keep only `construct`. The generated code
  reads every dependency before it builds a `#[default]` field, so a fallback never builds a default
  twice.

### Entries

Every unit of work that runs in a scope is an entry (#358). Entries are what entry plans (#348)
and openers (#346) key on, so all of them share one table regardless of where they come from.

Entry table, first slice of #358:

- An `EntryDescriptor` carries a stable namespaced `EntryKey`, the scope as a `ScopeKey`, its
  injected targets as dependency edges, and an optional owner (type plus operation) so tooling can
  group protocol entries under their controller or service (#360).
- Protocols push entries in `ProtocolDefinition::register`, plugins contribute them with
  provenance, and code that opens a scope itself registers with `scope_entry::<S, Deps>(key)`.
- Preparation sorts entries by key into an `EntryTable` and assigns dense `EntryId`s. It rejects
  duplicate keys, the transient scope, and scopes the protocol does not open.
- Each target must resolve from the entry's scope under the same rules component edges follow:
  reachable on every chain, a unique single provider, one config binding. Every candidate graph
  checks this again, so a reload that removes a target's provider is rejected.
- An entry references its scope's seeds rather than listing seed types; typed seeds are #347.

`#[entry]`, last slice of #358:

- `#[entry(scope = S)]` on a free function keeps the function and emits a unit marker type named
  after it in `UpperCamelCase`, carrying its `EntryDescriptor`. The parameters are the targets,
  plain `FromContainer` types as `#[init]` takes them, so entries stay protocol-neutral.
- The key defaults to `app/<function name in snake_case>`; `key = ".."` overrides it.
- Discovery collects every marker's descriptor from the `ENTRIES` slice, and
  `AppBuilder::entry::<Marker>()` registers one explicitly. A declaration registered both ways is
  kept once (same key and same recorded declaration).
- Under `di-check`, a required concrete `Arc<T>` target without a provider fails to compile at
  the parameter. Other targets are validated at preparation, as component fields are.
- Running an entry is the opener's job (#346, #405); the marker is what `.open(..)` and `.run(..)`
  will key on.


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

### Generation policy is part of the scope type

What happens to an open instance when a new runtime generation commits is declared by the scope
type, not by its boundary and not per opening (owner, 2026-10-28; #344):

```rust
#[non_exhaustive]
pub enum ScopeGenerations {
    Pinned,
    Durable,
}

pub trait Scope: Send + Sync + 'static {
    // ID, RANK, NAME, IS_TRANSIENT as today
    const GENERATIONS: ScopeGenerations = ScopeGenerations::Pinned;
}
```

| Policy | When a generation commits | Container | Typical scopes |
|---|---|---|---|
| `Pinned` (default) | Keeps the generation it opened under | One allocation, plain values, no `Live` | HTTP request, RPC call, WebSocket message, user units of work |
| `Durable` | Stays on its opening generation and reports how far behind it is | Slots ensured on first use | RPC and WebSocket connections |

- `ScopeKey` carries the policy, like `IS_TRANSIENT`. The topology, planner, containers, tooling and
  `#[scope(generations = durable)]` all read it from there. It replaces a boundary-level
  `ScopeLifetime`, and no scope ID is special-cased: a user scope gets the request treatment by
  declaring the same policy.
- It is fixed per type. Plans, slot layouts and the container kind are chosen when a generation is
  prepared, so a per-opening override would make every entry carry both container kinds.
- `Live` is a consequence of the policy, not a separate switch. A pinned instance is never rebound,
  so `Live` there would only cost an `ArcSwap` and a debt traversal on drop.
- `Dep<T>` and `Cfg<T>` keep the same types under every policy, so component code never sees which
  container it lives in. Pinned storage therefore needs a handle of the same type that is not backed
  by `ArcSwap`; this is settled before #345 fixes the slot layout.
- A pinned child of a durable parent uses the parent's generation.

Status (#344): the policy lives on `Scope`/`ScopeKey` with `Pinned` as the default; RPC
`Connection` and WebSocket `WebsocketConnection` declare `Durable`. Tooling projects it as each
scope resource's `generations` label. `AppRuntime::scope_generation` and
`AppRuntime::generations_behind` report the generation an open scope runs on and how far behind
it is. Every scope below the root stores fixed handles (no `Live`). Opening a scope no longer
copies the resolver set (each container answers its own `ComponentSource`), and a scope whose
layout captures nothing (no lazy, deferred, fresh or dynamic edge, no planned transient) is built
with an owned store and no pending resolver slot. Durable slots are still built when the scope
opens: building them on first use is deferred to #349, together with on-demand request slots, so
the synchronous accessors (`RpcCallContext::component`, `ScopeContainer::get`,
`ComponentSource`, `Deferred::get`) are settled once for every lazy slot. Dropping the second box
around fixed `Arc` handles is #425.
- The enum is `#[non_exhaustive]` so a `Rebind` policy can follow once rebinding exists: rebuilt
  onto each new generation in place, with `Live` storage. A `Rebind` scope may only open under the
  root or other `Rebind` scopes, since a rebound child under a pinned parent would mix generations.
  Container internals leave room for it; the variant is not exposed until it works.

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
- `ScopeOpener<S>` is the prepared opener (#346). Injected, it grants permission to open `S`, not
  to look anything up in it.
- `builder(params)` returns a scope builder: `.under(&parent)` for a typed parent
  (`S: OpensUnder<P>`), `.seed(value)` for declared seed slots, options such as `.capture_logs()`,
  then one of:
  - `.open(&entry)`: opens the instance with a registered entry's plan and returns it; protocols
    run their handler in it.
  - `.run(closure)`: a planned entry whose closure parameters declare its dependencies. The plan
    is cached per closure type.
  - `.enter(future)`: propagation only. The future runs inside the scope and reaches its values
    through declared `Current<T>` dependencies.

### One opening API for protocols and users

Protocols open their scopes through the same `ScopeOpener<S>` users inject (owner, 2026-10-28;
#346, #353). There is no second, protocol-only flow to keep in sync.

```rust
let opener: ScopeOpener<Request> = app.opener::<Request>()?;
let get_user = opener.entry::<GetUser>();

let scope = opener
    .builder(Request { id, method, route })
    .under(&connection)
    .seed(meta)
    .open(&get_user)
    .await?;
```

- A serve loop prepares one opener per scope it opens and resolves each entry's `EntryId` once.
  The declared scope is validated at that point, and parents are typed, so the per-call path
  does no topology lookup in release builds. It pins a generation (the current one under the
  root, the parent's own under a parent), takes the scope's layout from it and builds the
  container. `.open(&entry)` builds the slots the entry's plan lists; `.open_default()` builds
  every slot. Each seed must fill its own factory-less slot.
- The builder is a stack value with no allocation of its own.
- `OpensUnder<P>` is defined with the opener, because protocols need typed parents. Protocol scope
  types implement it by hand for the parents their topology lists; `#[scope(parents = [..])]`
  generates it (#405).
- The ad hoc `open_scope_*` API, with its per-call validation, is removed once the protocols move
  over, in line with the access rule above.

Status (#346, #353 HTTP): `AppRuntime::opener::<S>()` returns the `ScopeOpener<S>`, and
`ScopeOpener::entry(key)` resolves an entry once. `builder()` takes `.under(&parent)` (typed by
`OpensUnder<P>`, with `Singleton` standing for the root) and `.seed(value)`, then
`.open(&entry)` or `.open_default()`. The per-call path checks the parent topology only in
debug builds; a seed without a slot, or a second seed for the same slot, fails the opening in
every build. Params on `builder` arrive with `#[scope]` (#405), and `.open(&entry)` builds
what the entry's plan lists (#348, below). Axum's HTTP requests open through it: a hand-written layer keeps
the opener in reach of the request's task, and the scope opens on the first `Inject` or
`ScopeHandle` extraction (`ScopeHandle::of` for middleware holding the whole request), so a
request that injects nothing opens no scope. `RequestMeta` is seeded from the request head
when the scope opens. The request timeout is a hand-written layer too, so neither allocates a
boxed `from_fn` future.

Status (#353 RPC and WebSocket): the RPC serve loop prepares its connection and call openers once
per serve, and each call opens `.under(&connection)`. The WebSocket upgrade opens the connection
through an opener and hands protocols a `WsConnectionScope`, whose `message()` starts a message
scope under it; STOMP and JSON-WS open every message that way. The ad hoc `open_scope*` API is
removed, so `ScopeOpener` is the only way to open a scope. A scope that does not implement
`OpensUnder` for a parent cannot open under it; a hand-written impl the topology does not list
is caught by a debug assertion until #405 generates both from one declaration.

Status (#348, #349): preparation plans every entry against its scope's layout, per runtime
generation (`SlotLayout::plan_entry`, carried in `ScopeLayouts` and indexed by `EntryId`). A plan
lists the slots the entry's eager targets resolve to, by concrete type or every provider of the
trait, closed over their local dependencies (planned and unplanned edges alike). `.open(&entry)`
builds only those slots. Every slot is a once-cell, so a slot an opening left out is built on
demand by `ScopeContainer::resolve`, `resolve_qualified` and `extract` (which `Inject` and `Lazy`
go through): the slot and the local slots it reads, after ancestor slots it reads, once per scope
instance, each in its own async once-cell, on the instance's pinned generation. Synchronous reads
(`ScopeContainer::get`, `ComponentSource`, `RpcCallContext::component`) see only built slots.
HTTP routes find their entry by method and matched path, RPC calls by method path, WebSocket
messages by destination (`WsConnectionScope::open_message`); anything without an entry builds
every slot. Opening a scope first builds any enclosing-scope slot its planned factories read
that the enclosing scope's opening left out. Each on-demand slot is its own async once-cell, so
a factory built on demand may itself resolve another left-out slot of its scope. Tooling labels
each entry with the components it `builds`. Durable scopes still
build every slot when they open, and `deny_unplanned` strict mode is not implemented.

A request scope opened under an app-chosen parent, such as a per-user `Session` scope the
app caches and reuses across requests, fits this design: the parent is picked when the scope
opens (`.under(&session)`), never by re-parenting an open scope. It needs app-declared
parents for protocol scopes, so it follows #405.

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
