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

Without the macros, the same descriptors come from `const` builders: [`ComponentDescriptor::of`] with the `with_*` setters, the [`factories!`] macro over plain async factory functions, and [`ProviderDescriptor::of`] for a type that implements [`ProviderOf<dyn Trait>`](ProviderOf). Register them with the app builder's `component_descriptor` and `provider_descriptor`.

```rust,ignore
use upwell::{ComponentDescriptor, ConditionDescriptor, ConfigFactId, ProviderDescriptor, factories};

const MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");
static CUSTOM_MODE: ConditionDescriptor = ConditionDescriptor::config_equals("auth-custom", MODE, "custom");

async fn custom(db: Db) -> CustomAuthenticator {
    CustomAuthenticator::new(db)
}

static CUSTOM: ComponentDescriptor = ComponentDescriptor::of::<CustomAuthenticator>()
    .with_factories(factories![custom])
    .with_condition(&CUSTOM_MODE);
static CUSTOM_AUTH: ProviderDescriptor =
    ProviderDescriptor::of::<CustomAuthenticator, dyn Authenticator>("dyn Authenticator").primary();
```

A singleton that keeps state across a runtime transition declares a static replacement factory with a `replace` entry. Its first parameter is [`Previous<Self>`](Previous), an immutable snapshot of the instance being replaced. Every other parameter resolves from the candidate generation and must match a dependency of the ordinary factory: the same target, resolution mode and dynamic exemption, and never required where the ordinary factory is optional. When a config change replaces the component, the runtime calls this factory instead of the ordinary one. If it fails or panics, the previous instance stays active.

```rust,ignore
async fn carry(previous: Previous<Cache>, db: Db) -> Cache {
    Cache::with_entries(db, previous.entries())
}

static CACHE: ComponentDescriptor = ComponentDescriptor::of::<Cache>()
    .with_factories(factories!["fresh" => Cache::new, replace "carry" => carry]);
```

## Internal role

This is the engine the application layer drives. `upwell-dirs` implements [`Component`]/[`Injectable`] for its `Dir<K>` and `DirectoriesManager` against these traits. `upwell-config` seeds itself as an external resolver reachable via the [`ResolverCtx`], and `upwell-app` plus the protocol crates (`upwell-rpc`, `upwell-axum`) build [`ScopeContainer`]s, seed framework singletons (including the [`HookManager`](upwell_hooks::HookManager)), and resolve components through this crate.

## Feature flags

| Feature | Effect |
|---|---|
| `di-check` | Emit compile-time DI checks (`Wiring: Provide<Dep>` bounds) so a missing provider is a `cargo check` error rather than a runtime failure. |
| `serde` | Derive `Serialize` for the transition-planning vocabulary (`NodeAction`, `ReasonKind`, `TransitionReason`, `DependencyDemandId`, `EffectiveNodeRole`) used by redacted transition reports. |
