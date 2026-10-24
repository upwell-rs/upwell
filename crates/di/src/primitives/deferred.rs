//! `Deferred<T>`: a dependency hydrated once its scope finishes construction.

use std::sync::{Arc, OnceLock, Weak};

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode};

use crate::{
    ComponentConstructionContext, FromContainer, ScopeContainer,
    construct::{dependency_of, short_name},
    container::ScopeResolverSlot,
    error::Error,
};

/// Type-erased deferred slot hydrated after a scope finishes construction.
pub(crate) trait DeferredHydrator: Send + Sync {
    fn hydrate(&self, scope: &ScopeContainer) -> crate::Result<()>;
}

/// An Arc-specific deferred dependency retaining only a weak cached reference.
pub struct Deferred<T: ?Sized> {
    inner: Arc<DeferredInner<T>>,
}

struct DeferredInner<T: ?Sized> {
    qualifier: Option<&'static str>,
    value: OnceLock<Weak<T>>,
}

impl<T: ?Sized> Clone for Deferred<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Deferred<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    pub(crate) fn capture(
        scope: ScopeResolverSlot,
        qualifier: Option<&'static str>,
    ) -> crate::Result<Self> {
        let inner = Arc::new(DeferredInner {
            qualifier,
            value: OnceLock::new(),
        });

        scope.register_deferred(Arc::clone(&inner) as Arc<dyn DeferredHydrator>)?;

        Ok(Self { inner })
    }

    /// Returns the hydrated target when it is still alive.
    pub fn try_get(&self) -> Option<Arc<T>> {
        self.inner.value.get().and_then(Weak::upgrade)
    }

    /// Returns the hydrated target.
    ///
    /// Panics when called from a component factory before its scope has finished
    /// construction, or after the target's scope has been dropped.
    #[track_caller]
    pub fn get(&self) -> Arc<T> {
        self.try_get().unwrap_or_else(|| {
            panic!(
                "deferred dependency `{}` was accessed before scope hydration or after its scope was dropped",
                std::any::type_name::<T>()
            )
        })
    }
}

impl<T> DeferredHydrator for DeferredInner<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn hydrate(&self, scope: &ScopeContainer) -> crate::Result<()> {
        let value = match self.qualifier {
            Some(qualifier) => scope.resolve_qualified_built::<Arc<T>>(qualifier),
            None => scope.resolve_built::<Arc<T>>(),
        }
        .ok_or(Error::MissingComponent(short_name::<T>()))?;

        // Each slot is registered with exactly one scope and hydrated once;
        // a repeated set keeps the first value.
        let _ = self.value.set(Arc::downgrade(&value));

        Ok(())
    }
}

impl<T> FromContainer for Deferred<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        let mut dependency = dependency_of::<T>(Cardinality::One, false, false);

        dependency.resolution = ResolutionMode::Deferred;

        dependency
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        Self::capture(cx.resolver_slot(), None)
    }
}
