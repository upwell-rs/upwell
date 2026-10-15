# upwell-config

> Typed configuration for Upwell: format-agnostic loading plus a DI-integrated `Cfg`/`ConfigManager`/reload layer.

Part of the [Upwell](../../README.md) framework — the config layer, sitting above `upwell-dirs` and below `upwell-app` and the protocol crates.

## Role

This crate owns two layers. The **parser** is the format-agnostic core: a normalized [`ConfigValue`] tree, a placeholder grammar (`${KEY}` / `${KEY:default}`), a [`Resolver`] chain, and a [`from_value`] deserializer that resolves placeholders *while* deserializing. The **managed** layer integrates that parser with dependency injection: [`Cfg<T>`] injectables, the [`ConfigManager`] that loads and merges files, the [`ConfigStore`] resolver the DI container reaches config through, and the [`ConfigReloader`] staging engine that the application runtime's transactional reload drives. Substitution failures surface as [`TemplateError`]; load/bind failures as [`ConfigError`].

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. In practice you meet it through the `#[config("path")]` field attribute, which binds a `#[config]` type from the merged config tree and injects it as a [`Cfg<T>`]; `${VAR}` templating and `#[default]`s are resolved during deserialization, and live reload is requested through `upwell-app`'s `RuntimeReloader` (or `SIGHUP`/file-watch triggers), which stages config through the [`ConfigReloader`].

```rust
use upwell::{Cfg, config};

#[config]
#[derive(serde::Deserialize)]
pub struct GreetConfig {
    #[default = "Hello"]
    pub prefix: String,
}

#[service(id = "notifications", version = "0.1")]
pub struct Notifications {
    #[config("app.greet")]
    config: Cfg<GreetConfig>,
}
```

## Internal role

`upwell-app` builds on this crate to load and merge the config tree during app assembly, expose it through the DI container via [`ConfigStore`]/[`ContainerConfigExt`], and drive the [`ConfigReloader`] from its transactional runtime reload and reload triggers ([`ReloadTarget`]). The protocol crates (`upwell-rpc`, `upwell-axum`) and the `#[config]`/`#[service]` macros rely on the [`Cfg<T>`] injectable and the `CONFIG_BINDINGS` descriptor slice for compile-time binding discovery.

## Feature flags

| Feature | Effect |
|---|---|
| `toml` *(default)* | TOML config sources (via `toml`) |
| `yaml` | YAML config sources (via `serde_yaml_ng`), exposing `Yaml` |
| `watch` | watch config files and reload on change (`ConfigManager::watch_config`), via `notify` |
| `di-check` | compile-time DI graph validation (forwards to `upwell-di/di-check`) |
