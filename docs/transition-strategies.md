# Component transition strategies

A config reload can change which components are eligible and what they depend on. The planner
compares the active effective graph with the candidate graph and assigns every affected singleton a
minimum action: `Retain`, `RebindLive`, `Replace`, `Add` or `Remove`. Each affected component then
satisfies its action with a **strategy**. Keeping an instance whose own inputs did not change, or
rebinding its live `Dep<T>` cells, needs no declaration. Every strategy that runs component code
against a changed instance (hot update, transition handler, replacement) must be declared by the
component. The framework never guesses whether mutating an instance is safe.

This guide is the contract for those strategies. The worked example
[`examples/conditional`](../examples/conditional/src/lib.rs) uses live rebind, replacement and
reconstruction in one provider switch, in both directions, with a rollback. For the hooks that observe and gate a transition,
see [Transition hooks](transition-hooks.md).

## Declaring recipes

A component declares its recipes next to its ordinary factory with `factories!`:

```rust
static LEDGER: ComponentDescriptor = ComponentDescriptor::of::<Ledger>().with_factories(factories![
    "static" => construct_ledger,             // ordinary factory
    replace "carry" => replace_ledger,        // static state-aware replacement
    transition "pick" => pick_ledger,         // transition handler
    hot_update "entries" => update_ledger,    // explicit hot update
]);
```

With the macros, each recipe is a marked method in the component's `#[methods]` block. It
generates the same descriptor as the builder entry, with the ID `"<marker>:<method>"`:

```rust
#[methods]
impl Ledger {
    #[init]
    async fn construct(clock: Arc<Clock>) -> Self { /* ... */ }

    #[replace]                                 // replace "replace:carry" => Ledger::carry
    async fn carry(previous: Previous<Self>, clock: Arc<Clock>) -> Self { /* ... */ }

    #[transition]                              // transition "transition:pick" => Ledger::pick
    fn pick(change: &ComponentChange) -> Option<OneShot<Self>> { /* ... */ }

    #[hot_update]                              // hot_update "hot_update:entries" => Ledger::entries
    async fn entries(previous: Previous<Self>, clock: Arc<Clock>) -> HotUpdate<Self> { /* ... */ }
}
```

A method carries at most one marker, and a component at most one recipe of each kind.
Declaring any recipe on a non-singleton `scope` is a compile error, and so is a `#[replace]` or
`#[hot_update]` whose first parameter is not `Previous<Self>`. A hand-built descriptor with a recipe on a non-singleton
fails registry validation with `TransitionRecipeOutOfScope`.

## The strategies

| Strategy | Declared by | Instance afterwards | What runs before commit | Retires |
|---|---|---|---|---|
| **Retain** | nothing (unaffected components) | the same instance | nothing | no |
| **Live rebind** | nothing (`Dep<T>` consumers) | the same instance | the candidate value of each held live cell is prepared | no |
| **Hot update** | `hot_update "id" => f` | the same instance | `f(Previous<Self>, deps…)` returns an owned `HotUpdate` action | no |
| **Transition handler** | `transition "id" => h` | a new instance | `h(&ComponentChange)` picks a `OneShot` factory, which then builds the instance | yes |
| **Replacement** | `replace "id" => f` | a new instance | `f(Previous<Self>, deps…)` builds the instance | yes |
| **Reconstruct** | the ordinary factory | a new instance | the ordinary factory builds the instance | yes |

- **Live rebind** keeps a consumer whose only changes are live (`Dep<T>`) dependencies. The
  consumer keeps the live cell of the root that constructed it, and the commit republishes that
  cell with the candidate's value. This needs a single, eagerly resolved dependency on a singleton
  component or provider mapping. The consumer must be carried across generations through its
  snapshot adapter, and every target's live cell must be reachable. Provider mappings and typed
  component descriptors always are, even for a target declared `.without_retention()`; only a raw
  `manual` descriptor is not. Otherwise the consumer falls back to the next strategy below and is
  constructed again.
- **Hot update** keeps the instance. The preparation reads the instance and candidate dependencies
  and returns an action without mutating anything. The action runs inside the terminal
  publication, after the live rebinds. Only components stored as `Arc<Self>` can declare one, and
  it applies only while the component's identity, factory and dependency contract are unchanged.
  Its fixed dependents still reconstruct as planned.
- **Transition handler** receives the planned change. It returns a one-shot factory for this
  transition only, or declines so the next strategy applies. The one-shot factory may read
  `Previous<Self>` and may resolve only dependencies the ordinary factory declares. Any other
  dependency rejects the transition.
- **Replacement** builds the new instance from an immutable `Previous<Self>` snapshot of the
  active instance and candidate dependencies.
- **Reconstruct** runs the candidate descriptor's ordinary selected factory.

## How the runtime chooses

A reload decides per affected singleton, from the planned action and the component's
declarations:

1. `Retain`: keep the instance.
2. `RebindLive` whose every changed binding can rebind in place: keep the instance and rebind.
3. `Replace`, or a `RebindLive` that cannot rebind in place: the first declared of hot update
   (only while identity, factory and dependency contract are unchanged), transition handler,
   replacement factory, otherwise reconstruct. A declining handler falls through to the
   replacement factory or reconstruction.
4. `Add`: reconstruct.
5. `Remove`: retire by absence from the candidate root.

Every singleton the commit **constructs again** (handler, replacement, reconstruction) or removes
is displaced: it appears in `RuntimeTransitionReport::displaced()` and receives `ComponentRetired`.
That includes a rebound component that had to be constructed again. Retained, rebound-in-place and
hot-updated instances are never displaced.

To see these choices before changing anything, preview the change from tooling:

```sh
cargo upwell explain 'dyn my_app::Authenticator' --set auth.mode=default
```

The probe runs the same stage, evaluate and plan steps as a reload against the proposed config and
stops before construction. It prints each affected component's action, chosen strategy and reason
paths, the components a commit would displace, and why each provider mapping is or is not selected.
Trusted condition callbacks are not run, so a component they gate shows as not evaluated.

## Cost

A live rebind publishes the new instance into the consumer's existing slot: one atomic swap that
every `Dep<T>` holder observes on its next read. The consumer and everything that depends on it
keep their instances, so nothing downstream is planned, constructed or retired.

A reconstruction runs the component's factory again inside a new candidate root. Every fixed
dependent then has to be reconstructed as well, because it holds the old instance. The whole
reverse closure is planned, constructed, and displaced from the old generation at commit. Its cost
therefore grows with the size of that closure, not with the one component that changed.

Measured on an Apple M2 Pro (`just bench di_hot_paths di_graph_preparation reload_transition reload_allocations`):

| Work | Cost |
|---|---|
| Live slot swap (`live_slot_replace`) | ~70 ns per rebound slot |
| Constructing a component in a new root, framework overhead only (`construction`) | ~0.3 µs per component, plus its factory body |
| Planning a reverse closure (`reverse_closure/root`) | ~3 µs per component in the closure |
| Reload whose change leaves eligibility unchanged, 2048 components | ~9 MiB allocated; graph shared, not rebuilt |
| Reload with nothing staged | ~6 KiB allocated, independent of graph size |

A component that can absorb a change through `Dep<T>` therefore costs a reload about 50 times less
than one that must be reconstructed, before counting its factory body or its dependents. Prefer `Dep<T>` for dependencies that change at runtime and that have many dependents.

A reload whose re-evaluated conditions leave every component's eligibility unchanged shares the
base generation's effective graph and scope plans instead of rebuilding them. Planning then only
walks the reverse closure of the changed bindings. A reload that does change eligibility still
rebuilds the effective graph for the whole application.

## Scope

Transitions move **singleton instances only**. Request-, connection-, custom-scoped and transient
components have no instance in the root, so a reload never transitions them in place. It replans
them instead. When a change affects one (for example a request-scoped consumer of a switched
`Arc<dyn Authenticator>`), the planner still reports it, with its `FutureScope` or `Transient`
role, but it needs no strategy and is never constructed during the reload. The published
generation's scope plan builds it on the next opening.

Already open scopes **defer**. An open scope, and every scope opened beneath it, keeps resolving
from the generation it pinned until it closes. Only scopes opened from the root after the commit see
the new generation. The displaced generation stays alive (`AppRuntime::displaced_generations()`)
until the last such scope closes. Nothing ever migrates an open scope, and the reload never waits
for one.

The explicit `CandidateGraph::resolve_transition` path stays strict: a decision for a non-singleton
fails with `RestartRequired` (`NonSingleton`).

`RestartRequired` is also how every other unsupported request fails, always before construction.
Examples: a recipe the component did not declare, a `Previous` read for an added component, a hot
update whose contract changed, retaining a stale or generation-bound dependency, or a pre-built
instance with no transition contract.

## Failure, panics and cancellation

All fallible work completes before the new generation is published:

- planning and strategy validation;
- every factory, replacement, transition handler and one-shot factory;
- every hot-update preparation and live-rebind preparation;
- the `ConfigReload` and transition hooks.

The publication itself cannot fail.

If any of that work returns an error, panics, is rejected by a hook, or the reload future is
dropped, **nothing is applied**:

- the active generation, its instances, config slots and config generation stay as they were;
- every candidate instance built for the attempt is dropped with the candidate root;
- prepared live rebinds never touch their cells, and prepared hot-update actions are dropped without
  running;
- both serialization leases are released, so the next reload runs normally.

Panics in user code are contained and reported with a redacted category, never the payload. A
hot-update action that panics while being applied is the one exception that runs after publication:
it is contained and traced on `upwell::transition`, and the publication still completes.

The rollback covers framework-owned state only. Retained dependencies alias active instances, so
mutations made through them are visible immediately. External side effects (I/O, messages, global
state) are the application's responsibility.

## Identity and old generations

Each committed generation has a stable `RuntimeGenerationId`, and every attempt has a
`TransitionAttemptId`. A pinned `RuntimeView`, an open scope, or any other holder of an old root
keeps that generation's instances alive and usable after it is displaced. `Previous<Self>` and
retained handles are shared `Arc`s, never mutable aliases, so this is memory-safe. When the last
holder lets go, the old root drops with its retired instances. A `displaced runtime generation
released` event reports this, and `AppRuntime::displaced_generations()` lists generations still
waiting to be released.

## Design answers (#209)

- **Can a hot update be rolled back?** It never needs to be. Preparation only builds an owned
  commit action and mutates nothing. The action runs only inside a publication that can no longer
  fail, and an aborted attempt drops it unapplied.
- **How are async factories cancelled and cleaned up after a later prepare failure?** Candidate
  construction runs as ordinary futures owned by the reload attempt. A later failure, a rejection or
  a dropped reload future drops those futures, the candidate root and every instance built so far.
  Nothing outside the attempt refers to them before publication.
- **When are displaced instances notified and retired while old `Arc`s remain?** `ComponentRetired`
  fires once per displaced instance right after publication: logical displacement. Final
  destruction is reported when the old root is actually released.
- **May a transition-local factory alter declared dependencies?** No. It changes construction
  behaviour only, and may resolve only the dependencies the ordinary factory declares.
- **Which strategies are valid outside singleton scope?** None. Non-singleton components are
  rebuilt per scope from the generation the scope pins.
