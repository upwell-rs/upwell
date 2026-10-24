use std::{
    any::{Any, TypeId},
    sync::{Arc, Weak},
};

use upwell_core::{Resolver, ResolverCtx, ResolverSet, Scope};

use crate::descriptors::{Component, Injectable, component::ScopeStore};

mod candidate;
mod hot_update;
mod live;
mod open;
mod order;
mod resolve;
mod resolver_slot;
mod scope_registry;
mod snapshot;
mod transient;

pub use order::topological_sort;
pub(crate) use resolver_slot::ScopeResolverSlot;
pub use scope_registry::ScopeRegistry;
pub(crate) use transient::{
    construct_fresh_boxed, construct_transient, construct_transient_provider,
};

/// One scope's constructed instances, layered over an optional parent scope.
///
/// The root container is the singleton scope (`parent: None`); a per-connection
/// scope parents the root, and a per-request scope parents the connection.
/// Resolution walks this scope first, then each longer-lived parent, so a request
/// handler sees request-, connection-, and singleton-scoped instances uniformly.
///
/// The container is also a [`ResolverCtx`]: it exposes a [`ComponentSource`] (for
/// resolving components by type — used by hooks and config-targeted resolution) plus
/// any *external* resolvers (the config store) threaded in at build.
pub struct ScopeContainer {
    scope: &'static dyn Scope,
    store: ScopeStore,
    parent: Option<Arc<ScopeContainer>>,
    registry: Arc<ScopeRegistry>,
    resolver_base: ResolverSet,
    resolvers: std::sync::OnceLock<ResolverSet>,
    slot: ScopeResolverSlot,
    generation_lease: Option<Arc<dyn Any + Send + Sync>>,
    drop_guard: std::sync::OnceLock<Box<dyn Any + Send + Sync>>,
}

impl ResolverCtx for ScopeContainer {
    fn resolver(&self, kind: TypeId) -> Option<&dyn Any> {
        self.resolvers().resolver(kind)
    }
}

/// A [`Resolver`] over a [`ScopeContainer`], resolving components by type.
///
/// Held in the container's own resolver set under a [`Weak`] back-reference (so it
/// adds no reference cycle), it is how a hook reaches its `&self` receiver through
/// the erased `&dyn ResolverCtx` it is handed: `ctx.get::<ComponentSource>()?.component::<Self>()`.
pub struct ComponentSource {
    container: Weak<ScopeContainer>,
}

impl Resolver for ComponentSource {}

impl ComponentSource {
    /// The component of type `C` as its handle (`Arc<C>` or the by-value handle),
    /// resolved through the backing scope and its parents.
    pub fn component<C: Component>(&self) -> Option<C::Handle> {
        self.container.upgrade()?.get::<C>()
    }

    /// The handle `H` resolved through the backing scope and its parents.
    pub fn resolve<H: Injectable>(&self) -> Option<H> {
        self.container.upgrade()?.resolve_built::<H>()
    }
}

impl ScopeContainer {
    /// The scope this container holds.
    pub fn scope(&self) -> &'static dyn Scope {
        self.scope
    }

    /// The external resolvers threaded into this scope (config store, …), shared with
    /// child scopes.
    pub fn resolvers(&self) -> &ResolverSet {
        self.resolvers.get_or_init(|| {
            let container = self
                .slot
                .resolve()
                .expect("built scope resolver slot remains attached");
            let mut resolvers = self.resolver_base.clone();

            resolvers.insert(Arc::new(ComponentSource {
                container: Arc::downgrade(&container),
            }));

            resolvers
        })
    }

    pub(crate) fn registry(&self) -> Arc<ScopeRegistry> {
        Arc::clone(&self.registry)
    }

    /// Returns whether this container was built from `registry`.
    ///
    /// Runtime layers use this before opening a child to prevent containers from
    /// different prepared applications from being joined into one scope chain.
    pub fn belongs_to_registry(&self, registry: &Arc<ScopeRegistry>) -> bool {
        Arc::ptr_eq(&self.registry, registry)
    }

    /// Returns the runtime-generation lease retained by this scope when its type matches `T`.
    #[doc(hidden)]
    pub fn generation_lease<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        Arc::clone(self.generation_lease.as_ref()?).downcast().ok()
    }

    /// Retains `guard` until this container is dropped, then drops it after the
    /// container's own instances, so its `Drop` observes the scope's final release.
    ///
    /// A container retains at most one guard; a second one is handed back unchanged.
    #[doc(hidden)]
    pub fn retain_drop_guard<T: Any + Send + Sync>(&self, guard: T) -> Result<(), T> {
        self.drop_guard.set(Box::new(guard)).map_err(|rejected| {
            *rejected
                .downcast::<T>()
                .expect("a rejected drop guard is the guard just offered")
        })
    }

    pub(crate) fn can_access(&self, scope: &'static dyn Scope) -> bool {
        if scope.is_transient() || self.scope.id() == scope.id() {
            return true;
        }

        self.parent
            .as_ref()
            .is_some_and(|parent| parent.can_access(scope))
    }
}

/// Backwards-compatible alias: the root singleton store is a [`ScopeContainer`]
/// with no parent.
pub type ComponentContainer = ScopeContainer;

#[cfg(test)]
mod tests;
