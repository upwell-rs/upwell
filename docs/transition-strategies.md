# Component transition strategies

A config reload can change which components are eligible and what they depend on. The planner
compares the active effective graph with the candidate graph and assigns every affected singleton a
minimum action: `Retain`, `RebindLive`, `Replace`, `Add` or `Remove`. Each affected component then
satisfies its action with a **strategy**. A component gets a strategy that preserves its instance
only if it has declared that the strategy is safe. The framework never guesses whether mutating an
instance is safe.

This guide is the contract for those strategies. For the hooks that observe and gate a transition,
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

Attribute-macro sugar for the same metadata is tracked in #298.

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
  snapshot adapter, and every target's live cell must be reachable: provider mappings always are,
  and a component target is when its descriptor keeps its snapshot adapter. Otherwise the consumer
  falls back to the next strategy below and is constructed again.
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

## Scope

Transitions are **singleton-only**. Request-, connection- and other scoped components are never
transitioned in place. Each new scope is built from the generation that is current when it opens.
An already open scope keeps the generation it pinned. A planned transition that would have to move
a non-singleton fails with `RestartRequired` (`NonSingleton`) before anything is constructed.

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
