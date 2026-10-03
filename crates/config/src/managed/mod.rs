//! Configuration as a dependency-injection citizen.
//!
//! A [`ConfigManager`] (built in `main`) is split into small structs bound to property
//! paths: `app.db.reader` and `app.db.writer` may both deserialize the same
//! `DbConfig` type, injected wherever needed as [`Cfg<T>`]. Binding keys on the
//! property path — the same type may appear at several paths — with a type-only
//! shorthand that resolves only when exactly one binding of that type exists.

mod dirs;
mod reload;
mod source;
mod store;
mod trigger;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;

use upwell_core::TypeDescriptor;
use upwell_di::{BoxedComponent, Injectable, Live, LiveRef};

pub use dirs::DirectoriesResolver;
use reload::ConfigSlot;
pub use reload::{
    CONFIG_RELOADER_ID, CONFIG_RELOADER_NAME, ChangedBinding, ComponentHookReport, ConfigReload,
    ConfigReloadError, ConfigReloadReport, ConfigReloader, HookOutcome, ReloadProposal,
    ReloadableConfig,
};
pub use store::{ConfigStore, ContainerConfigExt};
pub use trigger::{spawn_reload_triggers, stop_reload_triggers};

#[cfg(test)]
mod tests;

use crate::DefaultSpec;

/// Link-time registry of every auto-registered config binding (a `#[config(path = "..")]`
/// type), collected by [`ConfigManager::auto_discover`].
#[linkme::distributed_slice]
pub static CONFIG_BINDINGS: [ConfigBindingDescriptor];

/// Runtime, object-safe access to a config type's [`DefaultSpec`].
///
/// [`ConfigProperties`] exposes its defaults as the associated `const`
/// [`DEFAULTS`](ConfigProperties::DEFAULTS), which is unreachable through a trait object (a
/// `const` is not part of the vtable). This companion trait — blanket-implemented for every
/// `ConfigProperties` — re-exposes that const through a `&self` method, so the spec can be
/// read from a value or a `dyn ConfigDefaults`.
pub trait ConfigDefaults {
    /// This type's field defaults (its [`ConfigProperties::DEFAULTS`]).
    fn defaults(&self) -> DefaultSpec;
}

impl<T: ConfigProperties> ConfigDefaults for T {
    fn defaults(&self) -> DefaultSpec {
        T::DEFAULTS
    }
}
#[cfg(feature = "yaml")]
pub use source::Yaml;
pub use source::{ConfigManager, Dynamic, Format, FormatId, ReloadTriggers, Toml};

/// Errors from loading, merging, and binding configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("failed to read config file '{}': {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file '{}': {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: crate::TemplateError,
    },

    /// The selected source file has no parser in the active format.
    #[error("unsupported config file format for '{}'", .path.display())]
    UnsupportedFormat { path: PathBuf },

    #[error("no configuration found at path '{path}'")]
    MissingPath { path: String },

    #[error("config substitution failed at '{path}': {source}")]
    Substitution {
        path: String,
        #[source]
        source: crate::TemplateError,
    },
}

/// An injected configuration value, bound from a property path.
///
/// Backed by a shared [`Live<T>`] slot, so a config reload swaps the value in place
/// and every holder observes it on the next read; snapshots taken earlier stay
/// pinned. The path is supplied at the injection site via `#[config("...")]` (or
/// omitted for the sole-binding shorthand). Read with [`get`](Self::get) (a guard)
/// or [`snapshot`](Self::snapshot) (an owned `Arc`).
pub struct Cfg<T> {
    live: Live<T>,
    path: Arc<str>,
    committed_snapshot: Arc<Mutex<source::ConfigSnapshot>>,
}

impl<T: Send + Sync + 'static> Cfg<T> {
    /// Wraps a freshly bound value with the property path it was bound at.
    pub(crate) fn new(
        value: T,
        path: impl Into<Arc<str>>,
        committed_snapshot: source::ConfigSnapshot,
    ) -> Self {
        Self {
            live: Live::new(Arc::new(value)),
            path: path.into(),
            committed_snapshot: Arc::new(Mutex::new(committed_snapshot)),
        }
    }

    /// A guard pinning the current value, dereferencing to `T`, for short reads.
    pub fn get(&self) -> LiveRef<'_, T> {
        self.live.get()
    }

    /// An owned `Arc` snapshot of the current value — stable once taken.
    pub fn snapshot(&self) -> Arc<T> {
        self.live.snapshot()
    }

    /// The property path this value was bound from.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Publishes a re-bound value into the slot. Used by a config reload at commit.
    #[allow(dead_code)]
    pub(crate) fn replace(&self, value: Arc<T>) {
        self.live.replace(value);
    }

    pub(crate) fn committed_snapshot(&self) -> source::ConfigSnapshot {
        self.committed_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn replace_snapshot(&self, snapshot: source::ConfigSnapshot) {
        *self
            .committed_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
    }
}

impl<T: Send + Sync + 'static> Clone for Cfg<T> {
    fn clone(&self) -> Self {
        Self {
            live: self.live.clone(),
            path: Arc::clone(&self.path),
            committed_snapshot: Arc::clone(&self.committed_snapshot),
        }
    }
}

impl<T: Send + Sync + 'static> Injectable for Cfg<T> {
    type Target = T;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// A **proposed** configuration value handed to a `#[hook(ConfigReload)]` method during a
/// reload, before it is committed. Read-only; derefs to `T`. If every hook accepts the
/// proposal the value is committed into its [`Cfg<T>`] slot, otherwise it is discarded.
pub struct CfgNext<T> {
    value: Arc<T>,
    path: Arc<str>,
}

impl<T> CfgNext<T> {
    pub(crate) fn new(value: Arc<T>, path: Arc<str>) -> Self {
        Self { value, path }
    }

    /// The proposed value.
    pub fn get(&self) -> &T {
        &self.value
    }

    /// An owned `Arc` of the proposed value.
    pub fn snapshot(&self) -> Arc<T> {
        Arc::clone(&self.value)
    }

    /// The property path this value was bound from.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl<T> Clone for CfgNext<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            path: Arc::clone(&self.path),
        }
    }
}

impl<T> std::ops::Deref for CfgNext<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

/// A struct bindable from a configuration subtree, injectable as [`Cfg<Self>`].
///
/// Implemented by `#[config]`; the type must also be
/// `Deserialize`. The default [`bind`](Self::bind) deserializes the subtree and wraps
/// it, so the builder can construct the value without naming the concrete type.
pub trait ConfigProperties: DeserializeOwned + Send + Sync + 'static + Sized {
    /// A display name for the type, used in descriptors and error messages.
    const NAME: &'static str;

    /// The type's `#[default = ".."]` field defaults, emitted by the `#[config]` macro as a
    /// compile-time constant.
    ///
    /// Defaults to [`DefaultSpec::None`](crate::DefaultSpec::None) — no fields
    /// carry a default. The values are template strings merged *under* the config so they
    /// resolve through the normal `${...}` pipeline (see
    /// [`ConfigManager::get_config`](crate::ConfigManager::get_config)). For runtime access
    /// through a value or trait object, use [`ConfigDefaults::defaults`].
    const DEFAULTS: DefaultSpec = DefaultSpec::none();

    /// Deserializes this type from the subtree at `path` (filling missing fields from
    /// [`DEFAULTS`](Self::DEFAULTS)) and wraps it as a stored `Cfg<Self>` handle.
    fn bind(tree: &ConfigManager, path: &str) -> Result<BoxedComponent, ConfigError> {
        let (value, snapshot) = tree.get_config_with_snapshot::<Self>(path)?;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<Self>(Self::NAME),
            value: Box::new(Cfg::new(value, path, snapshot)),
        })
    }

    /// Recovers a [`ReloadableConfig`] slot from a [`bind`](Self::bind) seed, sharing
    /// its live cell so a reload can re-publish the value in place.
    fn slot(
        tree: &ConfigManager,
        seed: &BoxedComponent,
        path: &str,
    ) -> Option<Box<dyn ReloadableConfig>> {
        ConfigSlot::<Self>::from_seed(tree, seed, path)
    }
}

/// A requested binding of a config type to a property path, registered on the
/// [`ConfigManager`] and resolved against the merged tree at build. The same type may be
/// bound at several paths; the `bind` thunk is monomorphized per type so the manager need not
/// name it. `defaults` is the type's compile-time [`DefaultSpec`], carried so the manager can
/// seed every bound type's defaults into the tree (enabling cross-path `${a.b.c}` references).
/// Monomorphized-per-type recovery of a [`ReloadableConfig`] from a bind seed, so the
/// type-erased manager can build reload slots without naming the config type.
pub type SlotThunk = fn(&ConfigManager, &BoxedComponent, &str) -> Option<Box<dyn ReloadableConfig>>;

#[derive(Clone)]
pub struct ConfigBinding {
    pub ty: TypeDescriptor,
    pub path: String,
    pub bind: fn(&ConfigManager, &str) -> Result<BoxedComponent, ConfigError>,
    pub slot: SlotThunk,
    pub defaults: DefaultSpec,
}

impl ConfigBinding {
    /// Builds a binding for type `T` at `path`, capturing `T`'s `bind`/`slot` thunks
    /// and compile-time defaults.
    pub fn of<T: ConfigProperties>(path: impl Into<String>) -> Self {
        Self {
            ty: TypeDescriptor::of::<T>(T::NAME),
            path: path.into(),
            bind: T::bind,
            slot: T::slot,
            defaults: T::DEFAULTS,
        }
    }
}

impl std::fmt::Debug for ConfigBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigBinding")
            .field("ty", &self.ty)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The link-time form of a [`ConfigBinding`]: a config type with a fixed property
/// path, registered by `#[config(path = "..")]` so it is picked up by
/// [`ConfigManager::auto_discover`]. The same type may still be bound at extra paths
/// explicitly via [`ConfigManager::with_config`].
/// It is also exposed on the type itself as
/// `Descriptor<ConfigBindingDescriptor>`.
pub struct ConfigBindingDescriptor {
    pub ty: TypeDescriptor,
    pub path: &'static str,
    pub bind: fn(&ConfigManager, &str) -> Result<BoxedComponent, ConfigError>,
    pub slot: SlotThunk,
    pub defaults: DefaultSpec,
}

impl ConfigBindingDescriptor {
    /// Lifts this link-time descriptor into a runtime binding.
    pub fn to_binding(&self) -> ConfigBinding {
        ConfigBinding {
            ty: self.ty,
            path: self.path.to_string(),
            bind: self.bind,
            slot: self.slot,
            defaults: self.defaults,
        }
    }
}
