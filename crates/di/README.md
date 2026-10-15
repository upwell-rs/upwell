# upwell-di

> The Upwell dependency-injection engine: scope containers, factories, and component descriptors over the core resolver.

Part of the [Upwell](../../README.md) framework — the runtime DI engine, sitting above `upwell-core` and `upwell-hooks`.

## Role

`upwell-di` owns the runtime DI machinery: the parent-linked [`ScopeContainer`], the construction-time [`Factory`]/[`FromContainer`] extractors, the component and provider descriptors ([`ComponentDescriptor`], [`ProviderDescriptor`]), and the [`ComponentRegistry`] that validates the graph. It builds on the leaf vocabulary in `upwell-core` (type descriptors, the dependency model, the resolver abstraction) and on `upwell-hooks` for the per-component hook slice each descriptor carries. Config is deliberately *not* here — it is an external resolver (`upwell-config`) reached through the [`ResolverCtx`](upwell_core::ResolverCtx), so the container stays unaware of it.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. You meet it through the `#[component]`/`#[service]` macros, which generate the [`Component`]/[`Injectable`] impls and register [`ComponentDescriptor`]s into the [`COMPONENTS`] distributed slice; field injection and `Inject<T>` resolve through the container it builds.

```rust
use upwell::prelude::*;

// `#[component]` generates the Component/Injectable impls and registers a
// descriptor; the DI engine wires `db` into `Store` at construction time.
#[component(by_value)]
#[derive(Clone)]
pub struct Db;

#[component]
pub struct Store {
    db: Db,
}
```

## Internal role

This is the engine the application layer drives. `upwell-dirs` implements [`Component`]/[`Injectable`] for its `Dir<K>` and `DirectoriesManager` against these traits. `upwell-config` seeds itself as an external resolver reachable via the [`ResolverCtx`], and `upwell-app` plus the protocol crates (`upwell-rpc`, `upwell-axum`) build [`ScopeContainer`]s, seed framework singletons (including the [`HookManager`](upwell_hooks::HookManager)), and resolve components through this crate.

## Feature flags

| Feature | Effect |
|---|---|
| `di-check` | Emit compile-time DI checks (`Wiring: Provide<Dep>` bounds) so a missing provider is a `cargo check` error rather than a runtime failure. |
| `serde` | Derive `Serialize` for the transition-planning vocabulary (`NodeAction`, `ReasonKind`, `TransitionReason`, `DependencyDemandId`, `EffectiveNodeRole`) used by redacted transition reports. |
