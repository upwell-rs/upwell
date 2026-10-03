//! The config store: a DI resolver holding every bound config value.
//!
//! Config values do not live in the DI container. They live here, in a
//! [`ConfigStore`] that implements [`Resolver`](upwell_core::Resolver) and is inserted
//! into the resolver set before the container is built. A `Cfg<T>` field or factory
//! parameter resolves through `ctx.get_resolver::<ConfigStore>()`, so the container stays unaware
//! that config exists — config is just another resolution source.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, DependencyDescriptor, Resolver, ResolverCtxExt};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, DependencyObservation, FromContainer, Injectable,
    ScopeContainer, dependency_of_observed, from_boxed,
};

use super::{Cfg, ConfigError, ConfigManager, ConfigProperties, ReloadableConfig};

/// A [`Resolver`] holding every bound config value, keyed by the config type and then by
/// its property path. Built before container construction and inserted into the resolver
/// set, so a `Cfg<T>` resolves through `ctx.get_resolver::<ConfigStore>()` rather than the
/// component container. Bindings are deduped per `(type, path)` upstream, so the inner
/// path map holds exactly one seed per path.
#[derive(Default)]
pub struct ConfigStore {
    by_type: HashMap<TypeId, HashMap<String, BoxedComponent>>,
}

impl Resolver for ConfigStore {}

impl ConfigStore {
    /// Binds every config registered on `manager`, producing the store and the reloadable
    /// slots (each sharing its `Cfg`'s live cell) the [`ConfigReloader`](super::ConfigReloader)
    /// drives. A bind failure aborts the whole build.
    pub fn build(
        manager: &ConfigManager,
    ) -> Result<(Self, Vec<Box<dyn ReloadableConfig>>), ConfigError> {
        let mut store = Self::default();
        let mut slots = Vec::new();

        for binding in manager.bindings() {
            let seed = (binding.bind)(manager, &binding.path)?;

            if let Some(slot) = (binding.slot)(manager, &seed, &binding.path) {
                slots.push(slot);
            }

            store.insert(binding.path.clone(), seed);
        }

        Ok((store, slots))
    }

    /// Records one bound value under its type and path.
    fn insert(&mut self, path: String, seed: BoxedComponent) {
        let type_id = seed.ty.type_id;

        self.by_type.entry(type_id).or_default().insert(path, seed);
    }

    /// The value of type `H::Target` bound at `path`, as handle `H`. Public so the
    /// field-injection macro can resolve a path-targeted `Cfg<T>` through the store.
    pub fn resolve_path<H: Injectable>(&self, path: &str) -> Option<H> {
        let seed = self.by_type.get(&TypeId::of::<H::Target>())?.get(path)?;

        from_boxed::<H>(seed)
    }

    /// The sole value of type `H::Target` (the by-type shorthand). `None` if zero or
    /// more than one binding of the type exists.
    pub fn resolve_sole<H: Injectable>(&self) -> Option<H> {
        let paths = self.by_type.get(&TypeId::of::<H::Target>())?;

        match paths.len() {
            1 => from_boxed::<H>(paths.values().next()?),
            _ => None,
        }
    }
}

/// A factory parameter of type `Cfg<T>` (the sole-binding shorthand): resolved through
/// the [`ConfigStore`] reached from the construction context. The path-targeted form is
/// emitted by field injection, which calls [`ConfigStore::resolve_path`] directly.
impl<T: ConfigProperties> FromContainer for Cfg<T> {
    fn dependency() -> DependencyDescriptor {
        dependency_of_observed::<T>(Cardinality::One, false, true, DependencyObservation::Live)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> upwell_di::Result<Self> {
        cx.get_resolver::<ConfigStore>()
            .and_then(|store| store.resolve_sole::<Cfg<T>>())
            .ok_or(upwell_di::Error::MissingComponent(T::NAME))
    }
}

/// Ad-hoc config access from a built container: the relocated `.config()` accessor.
///
/// Resolves a `Cfg<T>` bound at `path` through the container's [`ConfigStore`] resolver.
/// The DI container no longer carries config, so this convenience lives in the config
/// crate as an extension trait over [`ScopeContainer`].
pub trait ContainerConfigExt {
    /// The config value of type `T` bound at `path`, if present.
    fn config<T: Send + Sync + 'static>(&self, path: &str) -> Option<Cfg<T>>;
}

impl ContainerConfigExt for ScopeContainer {
    fn config<T: Send + Sync + 'static>(&self, path: &str) -> Option<Cfg<T>> {
        self.get_resolver::<ConfigStore>()?
            .resolve_path::<Cfg<T>>(path)
    }
}
