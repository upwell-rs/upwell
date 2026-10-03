# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(core)* add runtime generation identities and conditional dependency observation metadata

## 0.20.0 - 2026-07-22

### Added

- *(di)* dual linkme/inventory registration backend for factories & hooks

## 0.19.0 - 2026-07-21

### Added

- *(di)* add provider ordering and deferred primitives

### Fixed

- *(di)* resolve review findings in provider primitives
- *(di)* hydrate deferred dependencies after build

## 0.14.2 - 2026-07-17

### Other

- *(di)* harden hot paths and add controlled benchmarks

## 0.12.0 - 2026-07-08

### Other

- Added docs to all crates

## 0.7.0 - 2026-06-30

### Other

- Feature/protocol agnostic ([#20](https://github.com/upwell-rs/upwell/pull/20))

## 0.6.0 - 2026-06-26

### Other

- Config hot-reloading: Live/Dep, mutable Cfg, two-phase reload, hooks, and triggers ([#14](https://github.com/upwell-rs/upwell/pull/14))

## 0.5.0 - 2026-06-25

### Other

- manager owns the config registry and seeds all defaults (fixes cross-path default references) ([#12](https://github.com/upwell-rs/upwell/pull/12))

## 0.4.0 - 2026-06-25

### Other

- directory-namespace ergonomics + tagged-enum defaults ([#10](https://github.com/upwell-rs/upwell/pull/10))

## 0.2.0 - 2026-06-25

### Other

- directory namespace, templated field defaults, enum support, app errors & unified logging ([#6](https://github.com/upwell-rs/upwell/pull/6))

## 0.1.1 - 2026-06-24

### Other

- release v0.1.0

## 0.1.0 - 2026-06-24

### Added

- *(macros)* factory = path, default_factory = false; sunset #[derive(Component)]
- *(core)* Factory construction traits + #[methods]; #[init] unified onto them
- *(core)* per-type factory slices; factories own their dependencies
- *(macros)* replace #[derive(ConfigProperties)] with #[config] attribute
- *(core)* per-service RPC slices; services own their RPC surface
- *(core)* type→descriptor connection and by-type registration
- *(core)* tower-based middleware, guards, and a global error handler

### Fixed

- *(core)* give DirKind a display NAME and derive COMPONENT_ID from label
- fixed formating and fixed macro paths

### Other

- formatted code
- add crate metadata and internal dep versions for publishing
- Merge branch 'feat/11-middleware' into feat/builtins-middleware
- renamed the project to upwell
- Address PR #1 review: correct parse-error variant and cap resolution depth
- Add configuration system and application directories
- Optimize child scope allocation by reusing parent for empty scopes and conditionally seeding connection scope with PeerInfo
- Implement scoped dependencies with connection, request, and transient scopes
- Add support for streaming with custom codecs and enhance client call handling
- Name the components in the DependencyCycle error
- Add daemon! assembly macro, Wired graph check, and #[injectable] traits
- Add compile-time DI checking via Provide trait bounds (di-check feature)
- Add dyn-trait providers, qualifiers, collections, and by-value injection
- Refactor dependency management to use `linkme` for component registration and enhance dynamic dependency handling
- Add client SDK generation support and enhance error handling
- Implement structured status codes for RPC error responses
- Add streaming RPCs and Responder return-path migration
- Enhance DaemonBuilder with service registration and refactor component handling
- Add #[component] macro for system-constructed components and enhance field injection
- Implement Component metadata trait and refactor component registration
- Add support for RPC groups and enhance component resolution logic
- Add support for manual component registration and validation in the daemon
- Add overseer-macros crate and implement service and rpc procedural macros
- Add typed parameter extraction and connection-scoped state management
- Cleaned up the protocol layer
- Added rough transport and protocol impl
- Add tokio dependency and implement component lifecycle management
- Fixed formatting
- Fixed formatting
- Added basic core types and registry
- added core crate
