# upwell-core

> Leaf vocabulary for the Upwell framework: type descriptors, the dependency-edge model, scopes, and the resolver abstraction.

Part of the [Upwell](../../README.md) framework — the bottom of the dependency graph, the shared language every layer above speaks.

## Role

`upwell-core` is the leaf crate: it depends on nothing internal, and everything else depends on it. It defines the *vocabulary* the layers above use to talk about dependency injection without committing to a runtime — the by-type [`Descriptor`] and [`TypeDescriptor`] seam, the dependency-edge model ([`DependencyDescriptor`], [`Cardinality`]), component [`Scope`]s ([`Singleton`], [`Transient`], [`StaticScope`]), and the [`Resolver`]/[`ResolverCtx`]/[`ResolverSet`] abstraction through which all dependency resolution flows. It contains no container, no config, and no protocol code — those live in `upwell-di`, `upwell-config`, and the protocol crates.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. Its types surface indirectly: the `#[component]`/`#[service]` macros emit descriptors built from this vocabulary, and the resolver seam is what lets config (an external resolver) be reached without the container knowing about it.

```rust
use upwell_core::{StaticScope, Singleton};

// A scope marker describes a component's lifetime; the framework reads it off
// the descriptor rather than hard-coding a strategy.
fn describe<S: StaticScope>() -> &'static str {
    S::NAME
}

let _ = describe::<Singleton>();
```

Component conditions are [`ConditionDescriptor`] trees built with `const` builders (`config_bool`, `config_equals`, `component_eligible`, `provider_eligible`, `all`, `any`, `not`), each recording its call site for diagnostics.

## Internal role

Every higher crate builds on this vocabulary. `upwell-di` implements the runtime container against the [`ResolverCtx`] abstraction and reuses the descriptor/scope/dependency types. `upwell-hooks` is generic over [`ResolverCtx`] so a hook resolves its receiver without naming the DI container. `upwell-config` plugs in as an *external* [`Resolver`], and `upwell-dirs` and the protocol crates all speak this same type-descriptor language.

## Feature flags

None.
