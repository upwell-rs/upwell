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

`upwell::context::tokio::spawn` and the other propagation primitives (#132, #267) carry the
context and keep the scope instance alive until the spawned work finishes. A plain `tokio::spawn` or
`std::thread::spawn` carries nothing.

### One context per task, holding the whole chain

The task-local context holds the active scope chain (innermost instance up to the root), the
runtime generation it pinned, and invocation metadata. `Current<T>` resolves from the innermost
scope that can own `T`; `Scoped<S, T>` resolves only from scope `S`. Neither caches across
invocations. A missing context is a typed error, never a panic.

Status (#130, #266): `upwell_di::context::InvocationContext` is that task-local. It holds an
`Invocation` (a `#[non_exhaustive]` enum: HTTP request, WebSocket connection or message, RPC
call, job run, hook), the runtime as an opaque `ContextRuntime`, and the innermost scope
instance, which is the whole chain because containers link to their parents and which pins the
generation it opened on. A context with no scope resolves the root its runtime supplies:
`RuntimeView::context` pins the generation it was taken from (hooks and job runs), while
`ScopeOpener::context` follows the current one until a scope instance joins it. Lookups are
`pub(crate)` in `upwell-di`; outside every context they fail with `ContextError::OutsideContext`,
and with no scope instance with `OutsideInvocationScope`. Other crates get `#[doc(hidden)]`
hooks only: `enter(future)`, `capture()` and `bind_opened_scope`.

- **Lazily opened scopes.** HTTP opens the request scope on first injection, and RPC calls and
  WebSocket messages whose entry builds nothing open on first resolution. Their context is
  established first with a *pending* scope: the scope it expects and the parent instance it
  opens under. Until then the parent is the innermost instance; the opening calls
  `bind_opened_scope`, which fills a once-cell stored inline in the context. The task-local
  holds the context by value, so establishing it allocates nothing, unlike a shared per-request
  cell. Only the dispatching task sees the bind, which is where protocols open these scopes.
- **Capture and re-entry (for #132, #267).** `capture()` clones the active context (reference
  counts only) and `enter(future)` runs a future in it on any task. The clone holds its scope
  instance strongly. A capture taken before a pending scope opened keeps the pending state as
  captured, so propagation primitives should capture after the scope opens, or open it first.

Status (#133, HTTP and WebSocket): `RequestScopeLayer`, outside all global middleware, runs each
request in a context with a pending `HttpRequest` scope. It wraps the router's future, and the
router calls a route's layered services when that future is polled, so their synchronous `call`
is covered too. Code running before the scope opens sees the context without a scope;
global middleware sees the scope from the moment it or anything before it opens it, and from the
start with eager components. The context ends with the request's future, so a response body
polled later runs outside it, and the OpenAPI routes, mounted outside the layer, have none.
A WebSocket protocol's whole `serve` runs in a connection context in the connection scope. Each
message's handler runs in a nested message context (`WsConnectionScope::message_context`), which
shows the connection scope until a deferred message scope opens; the socket loop itself never runs
in a message scope, and an upgraded socket does not inherit the upgrade request's context.

Status (#268, RPC, jobs and hooks): each RPC call task runs in a context with a pending call
scope under its connection, around middleware, handler, response stream and the global error
handler; the call scope binds when it opens (at once for an entry that builds something, on
first resolution otherwise), so until then the connection scope is innermost. Each job run
runs in a `JobRun { job }` context holding the root the scheduler's `RootResolver` reaches, with
no scope; a scheduler whose root is gone runs the job without a context. Startup and shutdown
hooks, the reload transaction (proposal on the base, config-reload and decision hooks on the
candidate root), retirement and the transition outcome observers each run in a `Hook` context
pinned to the generation they address, with no scope.
Work a `ScopeBuilder::run` runs, its targets' resolution included, runs in a
`ScopeRun { scope }` context whose innermost scope is the instance the run opened.

**Correction (2026-11-06, #472):** The status descriptions above name the former closed
`Invocation` enum. The context instead holds an open `InvocationTarget`, and each owner defines
its own target: `HttpTarget`, `RpcTarget`, `JobTarget`, `HookTarget` and `ScopeRunTarget`.
`TargetRef` stores either a static reference or a shared `Arc`; HTTP, RPC, hook and scope-run
targets use static references, while a job target is shared across its runs. Inspection helpers
are available to framework tests through `upwell-di`'s `test-support` feature; no public dynamic
context lookup is added. The scope and generation behavior described above is unchanged.

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
- Running an entry is the opener's job (#346); `.open(&entry)` opens with the entry's plan, and
  `.run(closure)` (#405) plans its closure without an entry.


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

- **Through DI:** components in the scope inject `Arc<CustomerSync>` (implemented in #405).
- **Through the task-local context (planned, depends on #130):** `#[scope]` will generate
  `S::current() -> Result<Arc<S>, NoActiveScope>`. For a zero-sized scope it answers "am I inside
  one?", also offered as `S::is_active()`.

This refines the access rule. Reading the active scope's own value is allowed publicly, because it
builds nothing, is typed and closed, and each instance carries its own value. Resolving components
from the active container without a declared dependency stays non-public.

- Params are stored once per instance and shared by `current()` and injection, matching #344's
  single allocation per instance.
- Params are immutable, because one instance may run work in several tasks at once. Mutable
  per-instance state belongs in a scoped component.
- A `current()` read is not a dependency edge. It cannot build anything, so planning loses
  nothing, and tooling shows each scope's parameter fields from `#[scope]`.
- `CustomerSync { .. }.enter(future)` (planned with the invocation context, #130) will run a
  future inside just the params context, without an app, so code that calls `current()` can be
  unit-tested.
- The name "scope handle" is avoided while axum's `ScopeHandle` (an `Arc<ScopeContainer>` for
  dynamic access to a request's scope) exists; the value is plain `Arc<S>`.

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
| `Pinned` (default) | Keeps the generation it opened under | The container plus one slot block, plain values, no `Live` | HTTP request, RPC call, WebSocket message, user units of work |
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

Status (#344, container kinds): each `SlotLayout` records a container kind chosen from its scope
type's `GENERATIONS` when the generation is prepared; no scope ID is consulted. A pinned
container stores plain values in one block and allocates its on-demand cells on the first
left-out build; a durable container opens with one block of once-cells that every build fills,
concurrently per slot, and a failed build is not cached. Opening either with k planned slots
allocates the container and one slot block, plus what the k factories allocate (an allocation
test asserts this for k = 1, 4 and 16). Both kinds store fixed handles; a `Rebind` kind would
add swappable storage there. A displaced generation is released only once every scope of
either kind open on it has closed.

A root stores each singleton in a rooted slot holding the root's own instance. The cell a
`Dep<T>` shares is created on the first `Dep<T>` read, and a transition that rebinds it (a
retained consumer following a provider switch) no longer changes what the displaced root
answers: scopes still open on that generation keep reading its instance through `Arc<T>`,
on-demand builds and escaped `Lazy<T>` handles, while `Dep<T>` handles, including ones a
pinned component holds, follow the live binding. A message opened under an old-generation
connection runs on the connection's generation. `Cfg<T>` is not pinned yet: it reads the
committed config in every scope. One edge case is still open: a pinned scope that reads a
singleton's `Dep<T>` for the first time after its generation was displaced gets a cell the
commit never saw, so that `Dep<T>` stays on the displaced instance rather than following the
live binding. Whether `Dep<T>` in a pinned scope should pin or follow is to be settled in one
place, for both cases (#458).
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
            .try_run(|client: Arc<CustomerClient>| async move { client.sync().await })
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
  - `.run(closure)` / `.try_run(closure)`: a planned entry whose closure parameters declare its
    dependencies. The plan is cached per scope and parameter-type tuple. `run` takes work
    returning `()`; `try_run` takes work returning `Result<T, E>` and returns
    `Result<T, RunError<E>>` (`Scope(Error)` or `Work(E)`). The instance ends `Completed`, or
    `Errored { error_type }` when the work returned `Err` (only the type name, never the value),
    or `Failed` when the framework could not open it or resolve a target.
  - `.enter(future)` (planned with the invocation context, #130): propagation only. The future
    will run inside the scope and reach its values through declared `Current<T>` dependencies.

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
every build. `builder` takes the scope's value since #405, and `.open(&entry)` builds
what the entry's plan lists (#348, below). Axum's HTTP requests open through it: a hand-written
layer keeps the opener in reach of the request's task, and the scope opens on the first `Inject`
or `ScopeHandle` extraction (`ScopeHandle::of` for middleware holding the whole request), so a
request that injects nothing opens no scope. The request timeout is a hand-written layer too, so
neither allocates a boxed `from_fn` future.

Status (#347, request-bound seeds): `RequestMeta` is no longer copied into every request scope.
Axum owns the request head and consumes it before a handler's body runs, so the seed cannot be
derived on read; instead it is captured at most once per request, at the first of:

1. The scope's opening, when that opening reads it: the route's entry plan injects it, builds a
   component that depends on it, or reaches one lazily (`EntryPlan::reads_seed`); without an
   entry, when a factory of the scope (or of the eager set, for `open_on_demand`) depends on it.
   `ScopeBuilder::open_with_bound_seed` takes the seed as a closure and calls it only then.
2. An `Inject` of something the route does not declare that reads it
   (`ScopeContainer::extract_seeding`). It sees the head as it is at that point, so it includes
   rewrites middleware made after the scope opened. Before #347 it saw the head at the opening;
   this is the one reader whose values can differ.
3. Handing out a `ScopeHandle`, because a handle can resolve anything later, after axum has
   consumed the head. A handle therefore costs one header-map clone even if nothing reads
   `RequestMeta`. `ScopeHandle` is no longer a request extension; the open scope is kept in a
   private extension and reached through the extractor or `ScopeHandle::of`.

Filling a seed is not a construction, so `deny_unplanned` allows an undeclared read of
`RequestMeta` itself, while building an undeclared component that reads it still fails. Tooling
labels each scope with its `seeds` and each entry with the seeds it `reads-seeds`. The WebSocket
upgrade moves the upgrade head into `WebsocketUpgradeMeta` instead of cloning it; the connection
scope needs it at the upgrade, so it stays an eager seed there. RPC seeds `PeerInfo` only when a
component depends on it, which already avoids the cost.

A typed seed tuple per scope (`builder(params).seed((a, b))` checked at compile time) is not
done: a scope's seeds are not one fixed set per scope type. `WebsocketMessage` carries different
seeds per WebSocket protocol (STOMP seeds headers, session and principal; JSON-WS none), and RPC
seeds `PeerInfo` only when something depends on it. A per-scope tuple would force every opener to
supply the union. It fits better with `#[scope]` declarations (#405), where a scope's seed slots
are declared with the scope.

Status (#353 RPC and WebSocket): the RPC serve loop prepares its connection and call openers once
per serve, and each call opens `.under(&connection)`. The WebSocket upgrade opens the connection
through an opener and hands protocols a `WsConnectionScope`, whose `message()` starts a message
scope under it; STOMP and JSON-WS open every message that way. The ad hoc `open_scope*` API is
removed, so `ScopeOpener` is the only way to open a scope. A scope that does not implement
`OpensUnder` for a parent cannot open under it; a hand-written impl the topology does not list
is caught by a debug assertion; `#[scope]` (#405) generates both from one declaration.

Status (#126, protocol-controlled WebSocket connections): the upgrade no longer opens the
connection scope. `WebsocketProtocol::serve` receives a `PreparedConnection` (breaking): the
upgrade's head (`upgrade()`), negotiation metadata (`meta()`), the application root (`root()`),
`.seed(value)` / `.seeds(Seeds)`, and a consuming `open()` that returns the `WsConnectionScope`.
It cannot be cloned, so a connection opens at most once. `serve` runs in the connection's
invocation context from the start, as a context pending a `WebsocketConnection` instance; the
opened instance binds into it, so work after the opening sees the connection scope and
work before it sees none. The upgrade task keeps the admission permit; a protocol closes the
socket itself when the opening fails (`scope_failure_close()`). JSON-WS opens at once. STOMP
opens after `CONNECT` authentication: the authenticator resolves its arguments from the root,
reads the upgrade head through `StompConnect::upgrade()`, and returns `StompAuthenticated`
(principal plus `Seeds`), and the connection opens with them, so `StompPrincipal` is now a
connection seed instead of a per-message one. A connection-scoped authenticator argument is
rejected when the endpoint builds (`StompBuildError::ScopedAuthenticatorDependency`); any other
argument the root cannot resolve rejects the connection (`StompAuthenticationError::Unresolved`).
`upwell_app::Seeds` is the owned seed set a step that runs before an opening collects.

Status (#348, #349): preparation plans every entry against its scope's layout, per runtime
generation (`SlotLayout::plan_entry`, carried in `ScopeLayouts` and indexed by `EntryId`). A plan
lists the slots the entry's eager targets resolve to, by concrete type or every provider of the
trait, closed over their local dependencies (planned and unplanned edges alike). `.open(&entry)`
builds only those slots, stored as plain values. A factory-backed slot an opening left out is
built on demand by `ScopeContainer::resolve`, `resolve_qualified` and `extract` (which `Inject` and `Lazy`
go through): the slot and the local slots it reads, after ancestor slots it reads, once per scope
instance, each in its own async once-cell, on the instance's pinned generation. Synchronous reads
(`ScopeContainer::get`, `ComponentSource`, `RpcCallContext::component`) see only built slots.
HTTP routes find their entry by method and matched path, RPC calls by method path, WebSocket
messages by destination (`WsConnectionScope::open_message`); anything without an entry builds
every slot. Opening a scope first builds any factory-backed enclosing-scope slot its planned
factories read that the enclosing scope's opening left out: that exact slot and what it reads,
not other providers of its trait. A factory-less slot is never built on demand: if anything
resolves it, its seed must be supplied when its scope opens, or that resolution fails. A
resolution of one value builds only the component it names or the provider runtime selection
picks; a collection or keyed map builds every provider. Each on-demand slot is its own async once-cell, so
a factory built on demand may itself resolve another left-out slot of its scope. Tooling labels
each entry with the components it `builds`.

Status (#349, strict mode and deferred cycles): `AppBuilder::deny_unplanned()` (or
`deny_unplanned: true` in `app!`) sets `UnplannedConstruction::Deny` on the registry. Each
entry plan then also records the slots the entry declares: the ones it builds and the ones its
lazy and deferred targets reach. A scope opened for that entry fails with
`UndeclaredConstruction` when something builds an undeclared slot on demand. Scopes opened
without an entry (connection scopes, `open_default`) still build on demand. The slots one
on-demand resolution builds form a group whose deferred dependencies are hydrated once the
whole group is built, so a deferred cycle left out of an opening is built as a unit.

Status (#353, empty plans and connection scopes): `ScopeBuilder::open_when_needed(Some(&entry))`
returns an `EntryScope<S>`. When the entry's plan builds nothing it allocates no container:
reads that never build go to the parent, so they do not see the builder's seeds yet, and the
container opens with the seeds the first time something resolves from it, on the generation the
plan was read from. A deferred
opening that fails stays failed (`DeferredScopeFailed`). RPC calls and WebSocket messages open
this way, so a method or handler injecting nothing scoped opens no scope. RPC and WebSocket
connection scopes open with `open_on_demand()`, which builds only their seeds and eager set;
every other connection-scoped component is built the first time something resolves it.

Status (#350, construction policy): on demand is the default for every scope below the root,
and `#[component(scope = S, eager)]` opts a component out of it. `ComponentDescriptor` carries
the policy as `construction: ScopedConstruction` (`OnDemand` or `Eager`). Each `SlotLayout`
computes its eager set once, when the generation is prepared: every eager factory-backed slot
with the local slots it reads, closed over planned and unplanned edges like an entry's
targets. Every opening builds it: `plan_entry` starts each entry's plan from it (so it is
declared under `deny_unplanned` too), `EntryPlan::on_demand` (formerly `nothing`, used by
`open_on_demand` for connection scopes) builds exactly it, and `open_default` builds every
slot anyway. A plan holding the eager set is never empty, so `open_when_needed` opens such a
scope at once instead of deferring it. Axum's HTTP request scope otherwise opens only when
something injects from it; when the scope has eager components, a layer inside the global
middleware opens it for every routed request (#447). An application with no eager request-scoped
component does not get that layer. The macros reject `eager` without `scope` and with a scope path spelled
`Singleton` or `Transient` (spanned at the attribute), and a const assertion rejects any other
path naming the root or transient scope; registry validation rejects a hand-written eager
descriptor there with `EagerOutsideScope`. Tooling labels each component of a scope below the
root with its `construction-policy`, each scope with its `eager` set and, when an entry opens
it, the factory-backed components that no entry and no eager component reaches over selected
edges of any kind, across scopes (`unreached-by-entries`), and each entry with its
`plan-size`. `cargo upwell explain` shows these labels. An eager manual component has no
factory to run, so the macro rejects `eager` with `default_factory = false`, and a hand-written
factory-less descriptor is simply seeded.

#### Migration: on-demand construction (#348, #349, #350)

Before #348, opening a scope built every component of it, in one topological order. Now an
opening builds only what its entry needs, and the rest on first use. What changes for an
application:

- **Failures.** A failing `#[init]` (or factory) of a scoped component fails only the entries
  that use it, when they open their scope or first resolve it, instead of every opening of the
  scope. A call or request that never touches it succeeds. Mark the component `eager` to keep
  the old behaviour of failing every opening.
- **Ordering.** Components of a scope are no longer all built before the handler runs, and
  not in one fixed order across the scope: an entry builds its plan up front in layout order,
  and anything else is built when first resolved, possibly while the handler runs. A
  constructor must not rely on another component of its scope having been built unless it
  depends on it.
- **Side effects.** A constructor that exists for its side effect (an audit record, a span, a
  permission gate) no longer runs for entries that do not inject it. Mark it `eager`, or move
  the effect to middleware (axum `AxumMiddleware`, a `tower::Layer`). On HTTP an eager
  request-scoped component runs for every routed request before any middleware, as on RPC,
  so it runs for requests middleware rejects and reads the head as the client sent it. A
  request no route matches runs none. For an effect on every request, unmatched ones included,
  use global middleware.
- **Synchronous reads.** `ScopeContainer::get`, `ComponentSource` and
  `RpcCallContext::component` see only built slots; resolve through `Inject`, `Lazy` or
  `ScopeContainer::resolve` to build on demand.
- **Strict mode.** `deny_unplanned` turns undeclared on-demand builds into
  `UndeclaredConstruction` errors; eager components are always declared.
- **Hand-written descriptors.** `ComponentDescriptor` literals need the new `construction`
  field (`ScopedConstruction::OnDemand` keeps the default); `ComponentDescriptor::of` and
  `manual` set it.

Status (#354, extract fast path): `ScopeContainer::extract` first tries
`FromContainer::read_built`. For `Arc<T>`, `Dep<T>`, by-value handles and `Option<Arc<T>>`,
when `T` is already built in the scope chain and nothing provides `T` as a trait, the value is
read by its concrete type, with no provider selection and no construction context. Reading an
`Arc<T>` this way allocates nothing (a memory contract asserts this); a by-value handle may
still allocate if its `Injectable::from_stored` does.

Single-value trait targets (`Arc<dyn Trait>`, `Dep<dyn Trait>`, `Option<Arc<dyn Trait>>`,
unqualified or by qualifier through `resolve_qualified`) read through a per-layout table,
`SelectedSources`, planned once with the layout. It maps each trait to the source runtime
selection picks from that scope, as a `DepRef::Local`, `Ancestor` or `Root`. `extract`, `resolve`
and `resolve_qualified` read it first and fall back to selection when the table cannot answer:

- no entry: no provider is selected (none visible, or ambiguous), the selected provider is
  transient, or no planned slot holds it;
- the source is not readable on this chain: the enclosing scope is absent, or was opened without
  that layout;
- the slot is not built yet, or the scope itself was opened without a layout (the root, or a
  scope opened from a plain construction order rather than by an opener).

A scope that holds no components keeps its empty layout when it opens, which allocates nothing,
so it reads the table too. Before, it opened with an empty by-type store and selected per call.
That case is on the hot path: the RPC `Request` scope and a JSON-WS `WebsocketMessage` scope
declare no seed slots, so in an application without per-call components every trait `Inject`
in a call or message selected at runtime. The root stays layout-less: protocols never extract
from it per call (every `Inject` and handler parameter extracts from the per-call scope or, for
a deferred scope, opens it first), so planning a root table would not shorten a request path.

A layout plans with the scopes on *every* chain its scope opens in, but runtime selection sees the
chain it actually opened in. A scope opened under the root or a session therefore plans without
the session, yet selects the session's provider when opened under one. Each entry lists those
shadowing scopes (`shadowed_by`: unreachable at planning, holding a matching provider, tried
before the selected one), and a chain holding one falls back.

Collections and keyed maps (`Vec<Arc<dyn Trait>>`, `HashMap<String, Arc<dyn Trait>>`) read
through the same table: for each trait it also holds the providers a collection gathers from the
scope, in provider order, and the provider winning each qualifier of a keyed map, as `Local`,
`Ancestor` or `Root` sources. Its `shadowed_by` lists every scope the layout cannot reach on
every chain that holds a provider of the trait, because such a scope adds providers or changes
winners. It falls back like a single value, and also when any gathered provider is transient.
Reading allocates only the returned `Vec`, or the map and its keys (memory contracts assert both).

Planned entry parameters: `plan_entry` also pre-resolves each eager target an entry injects to
the source it reads (`EntryParam`), copied from the layout's tables, so reading one is a scan of
the entry's few parameters and a handle clone, with no lookup by type and no hashing.
`FromContainer::read_planned` only probes: it returns `Option<H>`, `None` when the plan holds no
parameter for the target or its source is not readable or built here.
`ScopeContainer::extract_planned(plan)` tries that probe and falls back to `extract` on `None`. A parameter records only sources a plan carried into a later
generation still names correctly: a slot of the entry's own scope (`plans_entries_like` keeps
slots and aliases) and a root singleton (carrying requires unchanged selection of every target).
A target in an enclosing scope has no parameter, since a carried plan could name a stale slot of
an enclosing layout that changed; it reads through the layout's table as before. A plan made for
one scope reads its own slots only from that scope, so the scope a deferred instance opens under
can answer a singleton target. `EntryScope` keeps its entry's plan, and its `extract` runs the
`read_planned` probe before it opens its container, and only opens it and calls
`extract_planned` when the probe returns `None`: an entry whose plan builds nothing and which
injects only singletons never opens its container. RPC `Inject` and WebSocket message handlers (whose generated code calls
`EntryScope::extract`) read through it.

Still open: HTTP `Inject` does not read planned parameters yet. Its request scope is opened in
`request_scope.rs`, which keeps the open container in a private request extension; keeping the
route's `Arc<EntryPlan>` next to it and calling `extract_planned` there is the remaining wiring.

Status (#351, allocation contracts): `upwell-rpc::allocation_contracts` and
`upwell::allocation_contracts` (both in the critical cohort) measure, per call on one thread,
the allocations and bytes of an RPC call, an HTTP request, a WebSocket message and a custom scope
opened through its `ScopeOpener`, and assert that 64 unused components in the call's scope change
neither what a call injecting nothing nor what a call injecting one component allocates, and that
an RPC call or WebSocket message injecting a planned singleton opens no scope. The plan-size
contracts needed a fix: an open scope's in-place slot block was sized to its whole layout,
128 bytes per slot, so every unused component cost a request that opened its scope 128 bytes.
Each entry plan now carries `SlotPositions`, the place of each slot its opening fills (the slots
it builds and the scope's seed slots), and the block holds only those; anything else is built on
demand into its own cell as before. This applies to pinned containers only: a durable container
keeps one once-cell per slot whatever its opening's plan, because it builds most slots on first
use, concurrently, and that block is opened per connection rather than per call. The contract
that an HTTP route injecting nothing allocates
no more than raw axum behind the same body limit is written but ignored: the request-scope layer,
applied as its own `Router::layer`, makes axum re-box every route, which costs 4 allocations and
about 400 bytes per request (the same as any `Router::layer`, `Identity` included), and one more
32-byte allocation is not attributed yet. RPC `Inject` and `EntryScope::extract` box the path that
opens a scope, so a handler future no longer holds that future inline: an RPC call injecting a
planned component went from 4 432 to 760 bytes.

Status (#359, planning at scale): entries are planned once per signature: the scope plus
the sorted set of target descriptors (type, qualifier, cardinality, resolution, observation,
optionality, dynamic and config flags; parameter names are ignored). Entries with equal
signatures share one `Arc<EntryPlan>`; `ScopeLayouts::entry(EntryId)` still answers per entry.
A reload works out the types whose provider selection each plan read: its non-config targets and
the targets of unplanned edges on the slots it builds or declares (`SlotLayout::entry_reads`).
A reload that changes eligibility plans the candidate from the active generation: a signature
keeps its plan by reference when the policy is unchanged, its scope's new layout plans entries
like the old one (`SlotLayout::plans_entries_like`: same scope, eager set, slot types,
provider aliases and local/collection/unplanned edge shapes) and no type it read is in
`ProviderSelectionModel::changed_selections` (providers, their order, qualifiers, primacy,
priority or component scope differ). Everything else is planned again; debug builds assert
that a carried plan equals a fresh one. A reload with unchanged eligibility still shares the
whole plan. Unique plans, per-boundary construction orders and same-rank layouts run on
`std::thread::scope` chunks once the work reaches 2 048 units (plans or components), in input
order, so results do not depend on the thread count; wasm and single-thread hosts stay
sequential. The threshold comes from the `entry_planning` bench: threads save 6-10% from about
5 000 components, nothing at 2 000, and cost 15% at 1 000 plans on a small graph. Preparing 10 000
components and 10 000 entries takes about 70 ms, mostly in serial work outside planning.

Status (#405, user-authored scopes, also #137): `#[scope(parents = [..], generations = ..)]` on a
unit or field struct generates, from the one parent list, the `Scope` impl, one `OpensUnder<P>`
and one const rank check per parent, and a `ScopeDeclaration` carried by the type
(`Descriptor<ScopeDeclaration>`) and appended to the `SCOPES` slice. Discovery and
`AppBuilder::scope::<S>()` register it, and `ScopeTopology::prepare_with` adds its boundary to the
protocol's topology, so the existing validation covers user scopes: a parent the topology lacks
and an identity another boundary holds are `ScopeTopologyError`s. `Root` is a type alias of
`Singleton`, and `ScopeParent::of::<Singleton>()` is `ScopeParent::Root`.

The decisions, and why:

- **Derived constants.** The ID defaults to `app/<type name in snake_case>` and the name to the
  type name. The rank defaults to one below the lowest-ranked parent: the rank only has to order a
  scope below its parents, since reachability, not rank, decides what a scope can see. `id`, `name`
  and `rank` override them.
- **Where params live.** A scope that is not zero-sized gets one factory-less slot of its own type
  (`ScopeDeclaration::params`, added to the registry at preparation). `builder(params)` fills it
  with a fixed `Live`, so `Arc<S>` reads it like any seed and needs no new container concept. A
  zero-sized scope gets no slot and stores nothing, so it has no value to inject: `Arc<Unit>` is
  rejected as a missing provider (at preparation, or at compile time under `di-check`). The
  check is `size_of::<S>()`, which also lets hand-written protocol scopes pass their unit value
  with no extra trait. `#[scope]` keeps its compile-time view on the same rule: a struct with fields must not
  be zero-sized (a const assertion rejects, say, `struct Flag { marker: () }`), so the
  `Provided<S>` it emits under `di-check` always has a slot behind it. A scope without data is a
  unit struct.
- **One builder.** `builder` now takes the scope's value; `.under`, `.seed`, `.open(&entry)` and the
  rest are unchanged, and protocols pass their unit scope. The builder's fields are private, so
  options such as log capture can be added without changing its type.
- **`.run` plans.** The work's parameters are its targets. The plan comes from the same
  `SlotLayout::plan_entry` as an entry's, is made on the first run and kept in `ScopeLayouts`, so it
  lives exactly as long as the layouts it was made from: a reload that reuses the generation's
  layouts keeps it, one that plans new layouts starts empty. It is keyed by scope and by the tuple of
  parameter types, not by the closure type: those types fully determine the plan, so closures with
  the same parameters share one, and a closure may borrow from its caller, which a `TypeId` of the
  closure would forbid. An unsatisfiable target fails the run with a `DiError` when it resolves;
  unlike a registered entry it cannot be checked at preparation, since the closure is unknown then.
  Work that should be validated up front registers an `#[entry]` and opens with `.open(&entry)`.
- **Injected openers.** `ScopeOpener<S>` is a `FromContainer` parameter, reached through the seeded
  `RuntimeReloader`. It holds the runtime weakly, so a singleton keeping one forms no cycle, and it
  can be injected while the application builds; it checks that the scope is declared when it opens
  an instance. It is not an `Injectable`, so it cannot be a `#[component]` field, and the planned
  `S: OpensUnder<C>` assertion for an injecting component is not generated.

Not done here: `S::current()`, `S::is_active()` and `.enter(future)` need the invocation context
(#130). `.run` is the seam: `ScopeWork::call` runs the work with the instance's container in hand,
and the instance's value is that container's `S` slot. Plugins cannot contribute a scope yet; a
`PluginContributions::scope` follows the `entry` contribution when a plugin needs one. Protocol
scopes keep their hand-written impls.

Status (#438, app-chosen parents): an application opens a protocol's request scope under a
longer-lived scope it owns, such as a per-user `Session` it caches, so the chain becomes
`Singleton -> Session -> HttpRequest` and session-scoped components are built once per session.
The parent is picked when the request scope opens, never by re-parenting an open scope, which
would leave components already built in the request resolved through the old chain.

- **Declaring it.** `#[scope(parents = [Root], generations = durable, children = [HttpRequest])]`
  (owner, 2026-11-08). From `children` the macro emits `impl OpensUnder<Session> for HttpRequest {}`
  (allowed for a foreign child because `Session` is local) and a const check that the child ranks
  below the scope, both spanned at the child, and records the child IDs on
  `ScopeDeclaration::children`. A child named twice is rejected at its second mention, and a path
  that is not a scope type fails the `Scope` bound there. `ScopeTopology::prepare_with` adds the
  scope as a parent of each child after the child's declared parents; a child no boundary
  declares is `ScopeTopologyError::MissingChild`, and a child that already lists the scope is
  `DuplicateParent`. It is additive only: a protocol scope keeps its own parents. The extended
  topology is validated on owned parent lists first; only a valid one interns its extended lists
  for the process, once per distinct list, so `ScopeBoundary` stays `Copy` and `const`
  constructible and a rejected preparation keeps nothing. A repeated `#[scope]` argument is a
  compile error at its second mention.
- **Opening under it.** `upwell_app::ParentScope<S>` is an open instance that `S` may open under,
  with the parent's scope type erased: `ParentScope::new(&OpenScope<P>)` requires
  `S: OpensUnder<P>`, and `ScopeBuilder::under_parent(&parent)` opens under it on the parent's
  generation, as `.under` does. The builder keeps its static parent type, so its opening methods
  still require `S: OpensUnder<P>` for it (the root by default). Protocol-neutral; only HTTP uses
  it so far.
- **HTTP.** `upwell_axum::RequestParent` is `ParentScope<HttpRequest>`, put into a request's
  extensions by a parent layer: `AxumAppBuilder::parent_layer(layer)` for a raw `tower::Layer`,
  or `parent_middleware::<M>()` for a DI-resolved `AxumMiddleware` such as the session cache
  holding its `ScopeOpener<Session>`. Parent layers are applied outside the global middleware and
  outside the eager request-scope opening (#447), inside the body limit and the timeout, so eager
  request components and `Current`/`Scoped` accessors see the session. They are applied with
  `Router::route_layer`, so a request no route matches takes the plain 404 path without
  reaching them. Whether a request opens at once is decided on the generation it opens on
  (`ScopeOpener::opens_at_once_under`): an old session's generation may still hold an
  accessor or eager component the current one dropped. The request-scope layer
  reads `RequestParent` only when a parent layer is registered: without one no extension is
  searched and every request opens under the root, as before; with one the cost is one extension
  lookup per opening plus the parent hop when resolving through the chain.
- **Planning.** `HttpRequest` now opens under the root or a session, so the session is not on
  every chain. Scope validation lets an optional edge (`Option<Arc<T>>`) name a target in a
  scope that encloses the consumer on at least one chain, since an enclosing scope cannot hold
  a value captive; a scope that never encloses it, such as a sibling, is still a
  `ScopeViolation`, since the edge could never be present. The topology computes each
  boundary's ancestors on any chain once at preparation (`PreparedScopeTopology::encloses`),
  and the DI validation entry points take that relation beside reachability. When the target is
  placed in several scopes, the edge validates when one placement encloses the consumer, in any
  registration order; slot planning reads that placement, since each layout is planned only
  against the layouts of scopes that may enclose it. Two placements that each enclose the
  consumer on different chains are rejected (`AmbiguousEnclosingPlacement`): the edge would
  have to resolve per chain (#490). The edge is `None` on a chain without the session and
  `Some` under one: a factory edge reads the session's slot as a planned ancestor, built on
  demand, and an injected target falls back through `shadowed_by`. Required edges are still
  rejected, so required dependencies on the session need an "only parent" mode (#487).
- **Generations.** A `Durable` session keeps its generation: a request under it runs on the
  session's generation, `AppRuntime::generations_behind` reports how far behind the session is
  after a reload, so the app's cache can evict it, and the displaced generation stays alive while
  the session holds it.
- RPC calls and WebSocket messages open under their connection; app-chosen parents for them
  (how a session relates to a connection) are a follow-up.

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

The `upwell.scope` span names the instance an event was made in, so routing covers every
`tracing` event, including events from third-party crates. The invocation context decides whether
that instance applies (#480): logs read the DI context, and propagation knows nothing about logs.

**Mechanism**

1. Opening a scope instance enters an `upwell.scope` span carrying the scope name and its `#[log]`
   params, and instruments the instance's future with it. Propagation primitives instrument
   spawned work with the current span as well as the context.
2. The span's extensions hold the instance's log sink.
3. Upwell's `ScopeLogLayer` (a `tracing_subscriber::Layer`, installed by `init_tracing` or added to
   a user's own subscriber) handles each event by walking the event's span stack to the innermost
   span with a sink, letting the invocation context decide whether it applies (see "Context
   decides membership" below), and pushing an owned `LogRecord` into the sink that does.
4. Capture tees: every event still reaches the user's normal output.

**Capture is off by default, and costs nothing until someone asks**

Capture is requested per instance, with `.capture_logs()` on the builder or by taking a stream
from the run (`run.logs()`); an instance that asks for neither has no sink and adds no per-event
work. With capture on, the sink starts records flowing on its first subscriber: an unwatched sink
costs a level check and one atomic read per event. Subscribing before the run starts sees every
record. Each subscriber has a bounded buffer and reports dropped records as a `Lagged(n)` item
rather than blocking the logging thread.

**Consuming logs as a stream**

```rust
let run = sync
    .builder(CustomerSync::from(customer))
    .try_run(|client: Arc<CustomerClient>| async move { client.sync().await });
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

- **Isolation:** an event only reaches the sink of the innermost capturing scope its span stack
  and invocation context agree on, so parallel instances never see each other's records, and work
  captured outside every invocation never logs into a capture it happens to run inside.
- **Completeness:** depends on propagation. Work spawned without the propagation primitives loses
  the span and the context, so its events reach only the global output: uncaptured, never
  misrouted.
- **Levels:** capture sees only events the layer's filter enables. A per-run capture level narrows
  it; going more verbose than the filter needs a more verbose filter on the layer, because
  enabling a callsite for one layer enables it for every layer.

## Further decisions

- A scope may be opened from more than one parent. The user declares every opening parent on the
  scope (`#[scope(parents = [Root, Request])]`), so the planner knows the finite set of chains and
  plans each one.
- The end-of-scope hook is observer-only. It fires after the instance's work finishes and before
  its log sink closes, so its own events are still captured.

## Decided at implementation (scope logs, #408, #409, #410)

1. **Loggable params are opt-in.** Only fields marked `#[log]` on a `#[scope]` struct are recorded,
   so credentials stay out of logs by default. The macro strips the marker and generates
   `Scope::record_log_params`, which reports each marked field (it must be `Debug`).
2. **Nested capture delivers to the innermost capturing scope only.** An instance that does not
   capture is transparent: its events go to the nearest capturing ancestor.
3. **Capture level defaults to the subscriber's.** `init_tracing` installs `ScopeLogLayer` with
   the same event filter as the console output. An error-level fallback admits the `upwell.scope`
   routing span without enabling additional captured events; `upwell::scope=off` takes
   precedence over this static fallback, though a dynamic selector can independently enable
   the span. Dynamic span selectors keep `EnvFilter`'s native semantics: for example,
   `upwell::scope[upwell.scope]=off` does not override the static fallback. A run's
   `.capture_level(..)` narrows what the event filter admits; it widens
   capture only as far as the layer's filter lets events through, because enabling a callsite for
   one layer enables it for every layer. Buffer size defaults to 1024 records per subscriber
   (`.log_buffer(NonZeroUsize)`; zero is unrepresentable, and a huge value is clamped to the channel
   maximum), unmeasured against #351 since optimisation is paused.

### Scope logs: surface and mechanism

**Surface** (module `upwell_app::logs`, facade `upwell::logs`)

- `ScopeLogLayer`: a `tracing_subscriber::Layer`, behind the `tracing-subscriber` feature.
  `init_tracing` installs it; users composing their own subscriber add it behind the filter that
  should bound capture.
- Builder options on `ScopeBuilder`: `.capture_logs()`, `.capture_level(level)`, `.log_buffer(NonZeroUsize)`.
- `ScopeBuilder::run` returns a `ScopeRun`, which implements `IntoFuture` (so `.await` works as
  before) and has `.logs() -> ScopeLogStream`. `OpenScope::logs()` and `OpenScope::span()` serve
  an instance opened with `open*` and `.capture_logs()`.
- `ScopeLogs`: an injectable parameter of `run` work with `.subscribe()` and `.is_capturing()`.
- `ScopeLogStream` is a `Stream<Item = ScopeLog>`; `ScopeLog` is `Record(LogRecord)` or
  `Lagged(n)`.

**Mechanism**

- Opening an instance creates the `upwell.scope` span (target `upwell::scope`, error level so
  restrictive filters still open it, fields `scope.id`, `scope.name` and the rendered `#[log]`
  params) and instruments the whole open-and-run future with it. Every open path makes the span,
  capturing or not (`run`, `open*`, immediate and deferred `open_when_needed`; the deferred one
  enters it when the container opens): without a subscriber that enables it the span is a
  callsite check, so protocol hot paths pay no allocation. Only an instance that captures gets a
  sink.
- The sink reaches the layer through a thread-local hand-off held only while the span is created:
  the layer's `on_new_span` takes it into the span's extensions. No dependency on the layer's
  crate is needed to create spans or sinks, and an instance whose span the layer never saw
  (no layer, or a filter that rejects the span) reports `ScopeLogSink::is_attached() == false`
  and captures nothing.
- `on_event` walks the event's span stack to the innermost span holding a sink and offers an
  owned `LogRecord` to every subscriber with `try_send`; a full buffer counts the record as
  dropped. The stream yields `Lagged(n)` once the buffered records have been read.
- **Context decides membership (#480).** Scope logs keep no ambient state of their own: there is
  no log task-local. A capturing instance retains its sink in its container's single drop-guard
  slot, set once right after the container opens (only builder-opened, non-root instances; a
  root's slot holds its generation's guard), and the sink keeps a weak link to the instance it
  opens under. `ScopeLogLayer` and `ScopeLogs::current()` resolve the same way from the innermost
  capturing `upwell.scope` span and the invocation context (read through a borrowing,
  `#[doc(hidden)]` `upwell_di::context::with_context_state`, which reports absent, masked or the
  entered context's scope chain):
  - **No context set at all** on the task or thread: the span decides, exactly as before. Logs
    work without the propagation machinery.
  - **Masked context** (work captured outside every invocation, run by a propagation primitive):
    no capture applies, and `ScopeLogs` is not capturing.
  - **Entered context without a scope instance** (a job's or a lifecycle hook's): the span
    decides, since there is no chain to check it against.
  - **Entered context:** the span's sink applies when its instance is on the context's scope
    chain, or opens under the context's innermost instance (so events made while an instance
    opens under the current invocation, eager construction included, stay captured). Otherwise
    the innermost capturing instance on the context's chain applies: a closure bound in
    invocation A and run inside B logs to A, and a task carried out of a `run` sees and logs to
    that run.
  - `ScopeBuilder::run` opens its instance inside its own context, pending under the instance's
    parent (`with_opening_scope`, which no `bind_opened_scope` claims), so the run's
    constructors log to the run wherever it was started, then runs the work in a context whose
    innermost instance is exactly the opened container. A deferred `EntryScope` of the same
    scope opened while the run's instance constructs cannot stand in for it. The protocols'
    claimable `with_pending_scope` keeps that window: a second lazy opening of the same scope
    under the same parent on the dispatching task binds first (follow-up: bind by the deferred
    instance's identity rather than scope and parent).
  - `ScopeLogs::current()` finds the current span's sink in a per-thread stack the layer keeps
    of the spans entered there while a sink is open (`on_enter`/`on_exit`), each with its
    innermost capturing ancestor's sink or none, so a detached root span entered inside a
    capture hides it, as it keeps its events out of it. It mirrors `tracing`'s span
    stack and decides nothing, but it locks no span extensions, so a formatter that holds them
    while rendering a value that asks for the log cannot deadlock, and it needs no concrete
    `Registry`, so it works with every `LookupSpan` subscriber the layer supports.
  - `ScopeLogs::current()` therefore reports where events made there are delivered: a
    non-capturing open inside a capturing run sees the run's capture (as its events reach it),
    and `is_capturing()` needs a layer that took the routing span, as capture does. It replaced the
    log task-local, whose answers could disagree with delivery.
  - A closed sink, whose instance finished, never applies, nor does one whose routing span no
    layer took (no layer, or a filter such as `upwell::scope=off` that rejects the span). The
    sink records which layer took it (an identity `ScopeLogLayer::on_layer` assigns), and a
    layer falls back to a chain instance's sink only when it took that sink itself, so events
    under another subscriber (`with_default`) never enter a capture that subscriber could not
    see. `ScopeLogs::current()` asks the current default dispatcher for its layer
    (`Dispatch::downcast_ref::<ScopeLogLayer>()`, which `Layered`, `Filtered`, boxed and `Vec`
    layers forward) and accepts only that layer's sinks, so a capture entered under one
    subscriber is not current under another. A `reload`-wrapped layer does not forward the
    downcast and reports no capture (wrap its filter instead); with several scope log layers in
    one subscriber, an unsupported setup in which each delivers, the first the downcast finds
    decides. Inside a `tracing` dispatch (a formatter rendering an event) `get_default` yields
    `Dispatch::none`, so there, as under an explicit `Dispatch::none` default, it accepts any
    layer's sink; spans of several subscribers interleaved on one thread can therefore make it pick another subscriber's capture (event delivery is unaffected; tracked in #484). While no sink is open anywhere an event costs one atomic read.
- **Cost of the rule:** an `OpenScope` span entered by hand inside an unrelated invocation's
  context routes to that invocation, not to the hand-entered instance, unless the instance opened
  under the invocation's innermost one. Run work inside the instance's own context (`run`, or a
  protocol's `EntryScope::establish`) to capture into it.
- A component sees the log of the code that resolves it: while its instance opens, that
  instance's. A component built lazily, via `open_on_demand`, or as an ancestor while a child
  instance opens, sees the log the resolving code routes to, not necessarily its own scope's.
  Routing construction to the owning scope's sink is a follow-up (#470); it interacts with
  protocol identity (#353).
- **Follow-up:** the container has one retained-value slot. If an instance ever needs a second
  retained value, replace the single slot with a small fixed struct of named optional fields
  (still static, no extension map).
- **End-of-scope hooks (#406):** two observer-only `#[hook]` kinds. `ScopeEnded<S>` fires when
  an instance of scope `S` ends (`#[hook(ScopeEnded<Session>)]`); `AnyScopeEnded` fires for every
  scope and reads `ScopeEnd::scope`. A hook takes a `ScopeEnd` (scope id and name, a
  `ScopeEndKind` of `Completed`, `Errored { error_type }`, `Failed`, `Cancelled` or
  `Released`, and the loggable params,
  never the container or any other field). Both fire for the same instance, typed first.
  - **Dispatch by type:** the kind's `TypeId` is already data in the hook descriptor, so the
    generic kind needs no new registration machinery; `ScopeEnded<NotAScope>` fails the `Scope`
    bound on the user's own token. `InstanceEnd<S>` is monomorphized per scope and fires
    `ScopeEnded<S>` straight from the generation's hook index.
  - **Allocation:** an instance holds an end only when a capture sink exists or `has::<ScopeEnded<S>>()`
    or `has::<AnyScopeEnded>()` holds for its generation, two O(1) index lookups when it opens.
  - **Ownership:** `InstanceEnd` lives with `InstanceLogs`. For `run` the run owns it and
    completes it with its outcome (a dropped or unwinding run ends as `Cancelled`). For an
    `OpenScope` or `EntryScope` (a protocol's request or connection scope) it is handed to the
    container's retained slot beside the sink, so the instance ends as `Released` when its last
    owner lets go of the container, `into_container` included. A scope that never opened emits
    nothing. The container's slot carries the sink for routing and an opaque keep-alive; the
    logs never look at the keep-alive and know nothing of hooks.
  - **Never inline:** the hooks run on a task spawned on the ambient Tokio runtime under the
    dispatcher that was current when the instance ended, inside the instance's span, and the task
    owns the `InstanceLogs` until they finish. They cannot delay or veto the code that ended the
    instance, cannot reach the instance to extend its lifetime or mutate it, and run after the
    work and before the sink closes. Without a Tokio runtime the end is traced at `warn` as
    skipped.
  - **Shutdown:** every runtime counts its pending end tasks. `App::run` and `App::serve` drain
    them for up to `END_HOOK_DRAIN` after the shutdown hooks, and `AppRuntime::drain_scope_ends`
    does the same on demand. A raw Tokio runtime dropped without either may still cancel them.
  - **Failures:** the hook manager isolates panics and errors; each is traced as
    `scope end hook failed` on `upwell::scope` with the owner and category, never the message or
    payload. No `catch_unwind` is added. `Failed` is a framework failure. `run` is `()`-only;
    a `try_run` work that returns `Err` ends as `Errored { error_type }`, recording only the
    type name, never the value.
  - **Decoupled:** logs and context propagation know nothing of hooks.

Delivery is always bounded and drops the newest record when a buffer is full, counting it in
`Lagged(n)`. An explicit `LogBuffer { Bounded(NonZeroUsize), Unbounded }` choice is a possible later
extension if wanted; `0` is never "unlimited".

Not wired yet: protocol dispatch (HTTP, WebSocket, RPC) opening its scopes through `open*` does
not request capture or instrument its handlers with `OpenScope::span()`; that follows with the
protocol identity fields (#353).

## Deferred to implementation

These are decided by the task that implements them, and recorded here when they are.

Nothing is open for scope logs; see the decisions above.

## One component type in several scopes (#119)

Owner decision, 2026-11-03: a single component type may belong to more than one scope, but the
scopes may not share a hierarchy. No scope of the set may be an ancestor of another on any
opening chain. This replaces the open candidate model of #119 (priorities, runtime conditions,
data-dependent effective scope) with a static one. "Nearest wins" shadowing across placements is
a possible later extension; the internals below keep the per-consumer lookup it would need, but no
user surface exists for it.

### Placements

- A **placement** is one scope a type builds in. A type has one placement per distinct scope of
  its explicit factories (`ComponentFactoryDescriptor::scopes`, empty meaning the component's
  declared scopes). With no explicit factory, the placements are the declared scopes: one, or each
  of a scope list (#136). The default field-injection factory builds in the declared scopes and is
  used only when the type has no explicit factory at all, as before.
- Two sources produce placements: factories with distinct scopes on one descriptor, and
  descriptors of one type declared in distinct scopes. `ComponentRegistry::resolved_components`
  expands both into one `ComponentDescriptor` per placement. Overrides apply per placement: a seed
  replaces the factory-built placement of its own scope only, and two descriptors of one type in
  the same scope are still `DuplicateComponentType`.
- A type with several placements gives each its own stable id, `<id>@<scope id>`, interned once so
  reloads do not allocate. The effective graph, hook owners, conditions and tooling all key
  components by id, so they need no change. A single placement in another scope than declared keeps
  the plain id. Expansion is idempotent: a placement records the scope it inherited
  (`declared_scope`) and expands to itself.

### Resolution (#135, #136)

- Selection is per app generation and per consumer scope, never per resolution: every lookup of a
  type by a consumer calls `ProviderSelectionModel::component_from(type, consumer, can_access)`,
  which returns the one placement the consumer can access. No overlap means at most one qualifies,
  so there is no ordering or shadowing rule to decide. When none qualifies, the first placement is
  returned so a violation names a real component.
- Each scope holds its own slot for the type. Layouts, the dense slot store and `ScopeRegistry`
  already key by scope, so a layout holds only its placement. `ScopeRegistry::from_placements`
  keeps one factory-backed entry per placement and `factory_backed(type, can_access)` picks the
  placement a `Fresh` request can reach.
- Trait providers map a concrete type, so a provider is visible to a consumer when any placement
  of its concrete type is accessible. Collections gather it once per consumer scope.
- Both factories of a type produce the same handle type by construction: handle and identity come
  from the `Component` impl, and only the scope varies.
- The cache key of a built instance is the scope instance, as before. An ancestor instance of the
  same type cannot exist by the overlap rule, so the "ancestor already has it" question of #135 does
  not arise. A selection cannot fail after dependents are built, because it is fixed at
  preparation.

### Scope lists (#136)

Status: done. `scope = [A, B]` is the list form of the placement model above, not a separate
fallback mechanism, so it answers the open questions of #136 as follows:

- `#[component(scope = [WebsocketConnection, HttpRequest])]` places the field-injection default in
  each listed scope, and `#[init(scope = [A, B])]` places that constructor in each. An `#[init]`
  without `scope` builds in every scope the component declares. A list of one is the plain form.
- The list is ordered only for the declared scope: the first entry is `DeclaredScope::Scope` and
  `ComponentDescriptor::scope`, and tooling lists placements in declaration order. Resolution is
  set-like, because the scopes may not share a hierarchy and a chain reaches at most one of them;
  there is no "first eligible open scope" rule and no shadowing.
- Construction stays per placement: on demand, or `eager` on every opening of each listed scope.
  Each placement has its own factory dependencies, checked against its own scope by the captive,
  provider and fresh validation. Singleton dependencies are shared by every placement.
- Descriptors: `ComponentDescriptor::scopes` lists every declared scope (empty for one, with
  `with_scopes` as the `const` builder), and `ComponentFactoryDescriptor::scopes` replaces the
  former `scope: Option<ScopeKey>` (empty inherits the component's scopes). Both are breaking
  changes to the descriptor literals.
- Diagnostics: an empty list and a path listed twice (aliases resolved, so `request` and
  `Request` are one) are compile errors at the offending token; `eager` checks every listed scope;
  `#[service]` and `#[controller]` reject a list like any non-singleton scope. Overlap, transient
  placements and seeds in two scopes fail preparation as before.
- Registry resolution checks each registered descriptor's lists by scope key: one scope named
  through two paths (or repeated in a hand-written list, the component's or a factory's) is
  `RepeatedScope`, and a non-empty `scopes` that does not start with the declared `scope` is
  `DeclaredScopeMismatch`. `with_scope` clears a list `with_scopes` set, and `with_scopes` sets
  `scope` to the first entry; both also drop a resolved placement's expansion record, so a
  placement registered again with new scopes is an ordinary descriptor (keeping its placement
  id). Tooling reports the two errors as `upwell/tooling-scope-repeated` and
  `upwell/tooling-declared-scope-mismatch`.
- Nearest-wins shadowing stays deferred.

### Validation and diagnostics (#138)

- `ComponentRegistry::validate_placement_overlap(components, encloses)` rejects a type placed in
  scopes where one encloses the other on any chain, the root included, with
  `Error::OverlappingPlacements` naming the type and both scopes. The app passes
  `PreparedScopeTopology::encloses`, a new "ancestor on some chain" query beside the existing
  "ancestor on every chain" `is_reachable`. It runs for startup preparation, topology validation and
  reload candidates.
- Two explicit factories building in one scope are `AmbiguousFactory`. A transient placement
  beside any other is `TransientPlacement` (transient is built wherever requested, so it overlaps
  everything). Captive-dependency, provider and fresh validation run per placement, because each
  placement is its own consumer with its own factory dependencies; nothing is checked
  conservatively across candidates.
- Duplicate priorities do not exist in this model; the equivalent, two factories in one scope, is
  rejected.

### Seeds and `Scoped<T>` (#139)

- A seed (factory-less descriptor) has one destination because `seed_destinations` and the seeding
  API address a type, not a scope. A type seeded in two scopes is `AmbiguousSeedScopes`. A type may
  be a seed in one scope and factory-built in a disjoint one.
- A scope list cannot make a seed several-scoped: `#[component(scope = [A, B],
  default_factory = false)]` is two seed slots of one type and fails with `AmbiguousSeedScopes`.
  A seed registered for one scope of a list replaces only that placement, so a type can be seeded
  in one scope and built in the others.
- `Current<T>` resolves the placement of `T` on the active chain; `Scoped<S, T>` resolves the
  placement `S` reaches and fails with `ScopedError::ScopeNotActive` where `S` is not on the
  chain. Neither needs to know that `T` has several placements.
- Status: done; the rules are tested in `crates/di/src/registry/tests/scope_lists.rs`,
  `tests/multi_scope_runtime.rs` and `tests/multi_scope_seed_list.rs`.
- `#[context]` values are the one exception (#127): a seed-only value declared with
  `#[context(scopes = [WebsocketConnection, HttpRequest])]` is a by-value, factory-less
  component with the `ScopedConstruction::Context` policy, placed in each listed scope through
  the scope-list machinery. `AmbiguousSeedScopes` is not raised when every seed placement of a
  type is a context; the overlap rule still keeps the scopes apart, so a chain reaches at most
  one slot, and `seed_destinations` lists every scope of a context. Each slot is filled by the
  builder of its own scope (`ScopeBuilder::seed`, `PreparedConnection::seed`,
  `StompAuthenticated::seed`, a `RequestSeeder`), which already addresses one scope. A context
  with a factory, or in the root or the transient scope, is `InvalidContext`.

Status (#128, seed claims): DI core knows only that a value is seeded from outside; the app
records who seeds it, without knowing any concrete protocol. There are two forms, by how many
openers a scope has:

- **A scope with one opener** (the HTTP request scope, a scope the application opens) is
  covered by plain claims, `SeedClaim { ty, scope, claimant }` on `AppRegistry::seed_claims`.
  Axum claims `RequestMeta` in `HttpRequest`, `AxumAppBuilder::request_seed::<T, M>()` claims
  `T` there as it registers the seeder, and `AppBuilder::provides_seed::<T, S>()` is the claim
  for a scope the application opens itself.
- **A scope several openers share** (every WebSocket endpoint opens `WebsocketConnection` and
  `WebsocketMessage`) is covered per opener. Each endpoint declares itself with
  `AppRegistry::declare_opener(OpenerSeeds { opener, scope, seeds })` (`"<protocol> at <path>"`):
  the upgrade metadata, plus what `WebsocketProtocol::seed_claims(&options)` returns. STOMP
  returns its headers and session for the message scope, its principal for the connection scope,
  and every `StompConfig::provides_seed::<T>()`; a STOMP endpoint that claims a seed without an
  authenticator is rejected while the application prepares (`WebsocketProtocol::check_options`,
  `StompBuildError::SeedWithoutAuthenticator`), so tooling reports it. Once a scope has a
  declared opener, plain claims do not count there (`WebsocketProtocol::register` says so), and
  a value is provided for a consumer only when **every opener that can reach that consumer**
  seeds it. An opener records the entries it serves (`OpenerSeeds::entries`: an endpoint serves
  the message handlers of the controllers speaking its protocol). An entry is reached by the
  openers serving it; a component by the openers serving any entry that transitively depends on
  it (over dependency edges and trait providers). An eager component, a component no entry
  reaches, and an opener that cannot be tied to entries (`entries: None`) fall back to every
  declared opener of the scope. The reviews of #496 found both holes this closes: with claims
  collapsed to `(type, scope)`, one endpoint's seed vouched for another endpoint's connections;
  with every opener checked for every consumer, a STOMP handler reading `StompHeaders` failed as
  soon as a JSON-WS endpoint was mounted. Reachability by entry is the smallest sound model:
  openers say what they fill and whom they serve, and nothing else changes.

Preparation (`validate_seed_claims`, beside the placement-overlap check) then rejects:

- a claim or an opener's seed for a scope without a factory-less slot of its type
  (`SeedClaimError::ClaimWithoutSlot`), since every opening that fills it would reject the seed;
- a required edge (eager or deferred, not optional, not configuration) of a component or an
  entry onto a checked seed whose placement on the consumer's chain is not provided: no claim
  covers it (`UnprovidedSeed`), or an opener that reaches the consumer does not seed it
  (`OpenerWithoutSeed`, naming that opener; tooling adds an `opener:<name>` resource). The
  placement is the one the consumer's chain reaches, so a connection
  seed covers consumers in message scopes below it. Checked seeds are every `#[context]` value
  and every type something claims; a plain factory-less slot no one claims keeps its old rules
  and fails only when resolved.

Several claimants of one `(type, scope)` pair are accepted: each describes an opener that fills
its own instances. Two fills of one opening are caught where the opening is assembled: two
request seeders of one type fail the axum build (`DuplicateRequestSeeder`), registering the same
seeder twice is a no-op, and any other double fill fails the opening (`DuplicateSeed`).

Tooling reports them as `upwell/tooling-seed-unprovided` and `upwell/tooling-seed-opener-missing`
(naming the consumer by component id or entry key), `upwell/tooling-seed-claim-without-slot` and
`upwell/tooling-context-invalid`, and labels a context's `construction-policy` as `context`. A
claim is a guarantee, not a check of each opening: an opener that claims a seed and opens a
scope without it fails the opening of an eager consumer, or the first resolution of an
on-demand one.

The REST path (#127): a `RequestSeeder` is a DI singleton producing the seed from the request or
rejecting it with a response. It runs for every request a controller route serves, inside the
parent layers and before global middleware and the eager request-scope opening, which covers the
same routes: a WebSocket upgrade opens no request scope, so neither runs for it (before, the
eager opening also opened a request scope for an upgrade); the seed waits in the request's extensions and is cloned into
the scope's slot when the scope opens, whichever way it opens. Seeders that are not registered
cost nothing: the opening reads extensions only when at least one is.

Use matrix:

| Need | Feature | Why not the others |
|---|---|---|
| Authenticated identity | A `#[context(scopes = [..])]` value (`Identity`), seeded by the protocol (`StompAuthenticated::seed`, a `RequestSeeder`) or `ScopeBuilder::seed` and claimed for each scope | It is produced outside DI and is immutable; a placement would construct it. |
| Protocol metadata (headers, peer) | A seed, lazily captured where declared (`RequestMeta`, `PeerInfo`) | Same as identity: the protocol owns its lifecycle. |
| Per-request optimised service | `#[component(scope = HttpRequest)]` | One lifecycle, one scope. |
| Per-connection optimised service | `#[component(scope = WebsocketConnection)]` | Same. |
| One service, per connection over WS and per request over REST | `#[component(scope = [WebsocketConnection, HttpRequest])]`, or one `#[init(scope = ..)]` per scope when construction differs | One type, disjoint lifecycles; no trait and no duplicate concrete types. |
| Singleton needing the current identity | `Current<Arc<Identity>>`, or `Scoped<S, Arc<Identity>>` to pin a scope | The singleton outlives every request, so it reads per invocation instead of holding. |
| Optional contextual value | `Option<Arc<T>>` on a consumer in or below `T`'s scope | Optional edges into a scope that encloses the consumer (#488). |

Multi-scope placement is not a workaround for missing protocol lifecycle control, and `Scoped<T>`
is not a workaround for a scope mismatch a placement fixes.

### Attributes (#140)

- `#[init(scope = <ScopePath>)]` sets the scope of that factory, with the same path and lowercase
  alias handling as `#[component]`; `#[init(scope = [A, B])]` sets several (#136). Omitting it
  inherits every scope the component declares.
  `#[component(scope = ..)]` is unchanged and scopes the default factory.
- A scoped `#[init]` emits a wrapper named after the marked function so several `#[init]`s on one
  type coexist across impl blocks; two unscoped ones still collide at compile time.
- `#[init]` parameters accept `#[config("path")]` and `#[qualifier = ".."]`, closing #115. The macro
  wraps such a parameter in a local type whose `FromContainer` reports the path or qualifier on the
  dependency edge and resolves through the config store or `resolve_qualified`. A bound parameter
  makes the constructor use the async path, because the ready path reads by type.
- Unknown `#[init]` arguments and a repeated `scope` are compile errors pointing at the argument.

### Open

- Two descriptors of one type declared in two scopes must carry distinct ids of their own;
  factory-expanded placements get `<id>@<scope id>`, which tooling shows.
- Nearest-wins shadowing is deferred; the list form is built as placements (#136).

## Access and propagation (#131, #132, #134, #267)

Track C of #403. It builds on the invocation context (#130, #266, #472) and follows the access
rule above: users get declared access and propagation, never a dynamic `current::<T>()`.

### Surface

Everything lives in `upwell_di::context`, re-exported by the facade as `upwell::context`.

| Item | What it does |
|---|---|
| `Current<T>` | A declared dependency. `get(&self).await` resolves `T` from the innermost scope instance of the active invocation, through its parent chain, so the closest instance wins. |
| `Scoped<S, T>` | A declared dependency pinned to scope `S`: it walks the active chain to the instance of `S` and resolves `T` from there, skipping any closer instance of `T`. |
| `ScopedError` | Why an accessor could not resolve. |
| `context::tokio` | `spawn`, `spawn_local` and `spawn_blocking`, carrying the context and the current `tracing` span. |
| `context::thread` | `spawn` and `scope`; every thread a `Scope` spawns carries the context. |
| `context::rayon` (`rayon` feature) | `join`, `scope`, `spawn`, `install` and the `ParallelIteratorExt::current()` adapter over the optional `rayon` dependency. Off by default. |
| `CapturedContext` | `capture()?` clones the active context and span for any other executor: `enter(future)` (a `JoinSet`, a custom executor), `run`, `bind` and `bind_fn` for synchronous code. |

Both accessors are zero-sized, `Copy` and stateless: constructing one captures nothing, and every
`get` resolves anew, so a singleton may hold one and see each invocation's own value. Neither
returns references into a container.

The propagation modules mirror the executors they wrap instead of a `CapturedContext::spawn*`
family, so the call site reads as the executor it targets. Each captures the context where it is
called; outside every context it carries only the span, masks any context the work happens to
run in (a rayon worker waiting inside another invocation's nested work, say), and otherwise
behaves as the function it wraps. `CapturedContext::capture()` is the strict form that fails with
`ContextError::OutsideContext`. The capture is the task-local `InvocationContext` itself, so the
`InvocationTarget` (and whether it is a static or a shared `TargetRef`) arrives unchanged.

### Typed errors

`ScopedError` is a `#[non_exhaustive]` `thiserror` enum:

- `Context(ContextError)`: no context (`OutsideContext`) or a context without a scope instance,
  such as a job run or a hook (`OutsideInvocationScope`).
- `ScopeNotActive { scope, name }`: `Scoped<S, T>` ran in an invocation whose chain has no `S`,
  such as `Scoped<Connection, _>` in a plain REST request. `scope` is the `ScopeId`, `name` its
  display name.
- `Unavailable { type_name }`: nothing on the chain provides `T`.
- `Resolution { type_name, source }`: building `T` failed, with the DI error as its source.

### Declared edges and validation

An accessor's `FromContainer::dependency()` is `T`'s own edge with `ResolutionMode::Current`, a
new resolution mode. `ResolutionMode` is now `#[non_exhaustive]`, with `as_str()` for tooling
labels (both breaking changes to the core vocabulary). The edge:

- is never planned, ordered or built at construction, and imposes no build order;
- is exempt from the captive-scope rule, since nothing is retained;
- still requires `T` to be provided by some registered component, provider or seed in some scope
  unless the edge is optional. A missing provider fails preparation with `MissingDependency`, so
  an accessor never makes a dependency silently optional;
- is reported by selection diagnostics as `invocation-context` and by tooling as `current`.

Planning accounts for these edges per scope, not per entry. When a generation is prepared, each
scope's `SlotLayout` computes its accessed set: the slots of that scope every `Current`/`Scoped`
edge in the registry resolves to (whatever scope its consumer lives in, already-built singletons
and enclosing or nested scopes included), with their dependencies there. Then:

- every entry plan of the scope declares the accessed set without building it, so building those
  slots on demand at `Current::get` is never an undeclared construction under `deny_unplanned`;
- every opening of the scope (an entry's, an everything opening, an eager-only opening) captures
  the seeds the accessed set reads, such as `RequestMeta`, so a late injection needs no extra
  check, whether the injected owner is already built or not;
- a scope with a non-empty accessed set opens at once: an RPC call or WebSocket message scope is
  not deferred (`EntryPlan::is_accessed`), and HTTP installs its at-once opening whenever a
  registered component declares an accessor and opens routed requests while the current
  generation's request layout is accessed;
- the set is part of the layout, so the old-versus-new layout comparison plans every entry of the
  scope again when a reload changes it (an accessor component enabled or disabled anywhere).

In a slot layout an accessor edge plans to `DepRef::Contextual`, which is never built with its
consumer: the value is built at `Current::get`, on demand. A `Scoped<S, T>` edge does not carry `S`,
so it is marked in every scope that provides `T`; carrying the pinned scope on the edge is a
follow-up.

The trade-off is precision: in an application where an accessor targets a scope, every entry of
that scope opens it at once and captures the seeds the accessed values read, whether or not that
entry reaches the accessor. Applications without accessors plan, open and capture exactly as before.
An earlier per-entry walk was replaced because it had to be repeated at every decision point
(built-value injection, eager components, eager-only openings, reload carry-over) and could not
declare targets in enclosing scopes.

When a protocol never opens the scope an invocation waits for, an accessor that cannot find its
target on the open chain fails with `ScopedError::ScopeNotActive` naming that scope, never a
panic.

The macro rejects `#[qualifier]` and `#[config]` on a `Current<..>` or `Scoped<..>` field with a
compile error at the attribute: an accessor resolves its target's unqualified provider, so a
qualifier would otherwise be dropped silently.

The captive-scope error (`ScopeViolation`) names `Current<T>` as the way for a longer-lived
component to read a shorter-lived value.

Not validated at preparation: that `S` of `Scoped<S, T>` can be on a chain where the consumer
runs, and that `T` is reachable from `S`. The edge has no slot for `S`; both fail at run time
with a typed error. Follow-up: carry the pinned scope on the edge.

### Propagation semantics

- Explicit only: a plain `tokio::spawn`, `spawn_blocking` or `std::thread::spawn` carries nothing.
- A capture holds its scope instance (and so the whole chain and its pinned generation) strongly
  until the captured work ends.
- Cancellation is the executor's own, and nothing is tied to the response's completion. Dropping
  a tokio `JoinHandle` detaches the task rather than cancelling it, and aborting a
  `spawn_blocking` task that already runs does not stop it; threads cannot be cancelled at all. A
  detached task therefore extends the life of a request's components, so request-bound work must
  be awaited (`JoinSet`, `join!`, `context::thread::scope`) or its handle managed explicitly.
- A capture taken before a pending scope opens keeps the pending state, so capture after the
  handler has resolved from its scope (or injected anything from it).
- Synchronous code sees the same context as async code: tokio's task-local is a thread-local
  while a scope is entered, so `run`/`bind` on any thread set exactly the storage a future reads,
  and `Handle::block_on(current.get())` inside them resolves from the captured context.
- Spawned work is also instrumented with the span that was current at the capture, so its events
  keep their scope's log routing.
- Every spawning function documents its `# Panics` (outside a runtime or `LocalSet`, thread
  creation failure, rayon's global pool failing to start).

### rayon

rayon has no per-job hook, so the context travels per job: `join`, `scope`, `spawn` and `install`
bind every closure they hand to the pool, and a rayon `Scope` hands its jobs a scope that binds
their nested spawns too. `.current()` returns a `WithContext` that is deliberately not a
`ParallelIterator`: it offers `map`, `filter`, `filter_map`, `inspect`, `fold`, `for_each`,
`reduce`, `all`, `any`, `count` and `collect`, and runs every closure passed to them inside the
context, whichever worker steals it. Wrapping rayon's consumer instead cannot follow the
consumer rayon hands a nested iterator (`flat_map`, `flatten`), so those adapters are not
offered; nested parallel work started inside a bound closure carries the context by calling
`.current()` or a `context::rayon` function there. Adapters before `.current()`, indexed ones
(`zip`, `enumerate`) included, run outside the context. Each bound job sets the context on the worker
thread only for its own run and restores what was there, so bound jobs never see each other's
contexts. The remaining hazard: a bound closure that itself blocks in rayon (a nested `join`,
`scope` or parallel iterator) may run other pool jobs on its thread while it waits, and a job
there that was not bound sees the waiting job's context. When the context selects identity,
tenant data or credentials, bind every job of a pool shared with context-carrying work, or give
such work its own pool and `install` into it.

### `S::current()` and `S::is_active()`

`#[scope]` generates `S::is_active()` for every scope and, for a scope with params,
`S::current() -> Result<Arc<S>, ScopedError>`, both reading the active chain. Reading the active
scope's own value is the public exception above: it builds nothing and is not a dependency edge.
`S { .. }.enter(future)` (a params-only context for unit tests, without an app) is not done: it
needs a context form with a value but no container. Follow-up.
