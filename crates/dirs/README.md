# upwell-dirs

> Unified application directories for Upwell, injectable as `Dir<K>`.

Part of the [Upwell](../../README.md) framework — a standalone directories layer on top of `upwell-core` and `upwell-di`.

## Role

`upwell-dirs` provides an application's platform directories as typed injectables. A [`DirectoriesManager`] — built once (typically in `main` or by the app builder) from project metadata — resolves the per-application config, data, cache, state, runtime, and temp directories via the `directories` crate's XDG / Known-Folder logic. Each is handed out as a typed [`Dir<K>`] keyed by a marker ([`Config`], [`Data`], [`Cache`], [`State`], [`Runtime`], [`Tmp`]), each implementing [`DirKind`]. `Dir<K>` derefs to its [`Path`] and reading it never fails (resolution happened once at the manager); only [`ensure`](Dir::ensure) touches disk. The crate is standalone: it depends only on `upwell-core` and `upwell-di` and knows nothing about config.

## Usage

Most users depend on the [`upwell`](../../README.md) facade, which re-exports this crate — you rarely name it directly. A component field-injects exactly the directory kind it needs:

```rust
use upwell::prelude::*;
use upwell::dirs::{Dir, Data};

#[component]
pub struct Store {
    data: Dir<Data>,
}

impl Store {
    fn db_path(&self) -> std::path::PathBuf {
        self.data.join("store.db")
    }
}
```

## Internal role

`Dir<K>` and `DirectoriesManager` implement [`Component`](upwell_di::Component)/[`Injectable`](upwell_di::Injectable) from `upwell-di`, so they resolve through the DI container like any other component (and under `di-check` are framework-seeded via [`Provided`](upwell_di::Provided)). `upwell-config` consumes [`DirectoriesManager::entries`] — the directory namespace as plain `(label, path)` data — to build a `${@config}`/`${@runtime}`/… templating resolver without ever naming the individual `Dir` kind types. `upwell-app` seeds the manager so directory resolution is defined once for the whole application.

## Feature flags

| Feature | Effect |
|---|---|
| `di-check` | Forwards to `upwell-di/di-check`, enabling the framework-seeded [`Provided`](upwell_di::Provided) impls so `Dir<K>` and `DirectoriesManager` satisfy compile-time DI checks. |
