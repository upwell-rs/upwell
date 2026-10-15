//! Scope-capturing deferred and forced-construction dependency handles.

use std::{
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, OnceLock, Weak},
};

use arc_swap::ArcSwapOption;
use tokio::sync::Mutex as AsyncMutex;
use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode};

use crate::{
    ComponentConstructionContext, FromContainer, Injectable, ProviderDescriptor, ScopeContainer,
    construct::{dependency_of, short_name},
    container::{ScopeResolverSlot, construct_fresh_boxed},
    descriptors::component::from_boxed,
    error::Error,
};

/// Type-erased deferred slot hydrated after a scope finishes construction.
pub(crate) trait DeferredHydrator: Send + Sync {
    fn hydrate(&self, scope: &ScopeContainer) -> crate::Result<()>;
}

/// A cloneable, strongly cached dependency resolved from the consumer's scope.
pub struct Lazy<H> {
    inner: Arc<LazyInner<H>>,
}

struct LazyInner<H> {
    scope: ScopeResolverSlot,
    resolver: LazyResolver<H>,
    value: ArcSwapOption<H>,
    initializing: AsyncMutex<()>,
}

type LazyResolver<H> = Arc<
    dyn Fn(Arc<ScopeContainer>) -> Pin<Box<dyn Future<Output = crate::Result<H>> + Send>>
        + Send
        + Sync,
>;

impl<H> Clone for Lazy<H> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<H> Lazy<H>
where
    H: FromContainer + Clone + Send + Sync + 'static,
{
    pub(crate) fn capture(scope: ScopeResolverSlot) -> Self {
        Self {
            inner: Arc::new(LazyInner {
                scope,
                resolver: Arc::new(|scope| Box::pin(async move { scope.extract::<H>().await })),
                value: ArcSwapOption::empty(),
                initializing: AsyncMutex::new(()),
            }),
        }
    }

    /// Returns the cached value without resolving it.
    pub fn get(&self) -> Option<H> {
        self.inner.value.load_full().map(|value| (*value).clone())
    }

    /// Resolves normally from the captured scope and replaces the cache.
    pub async fn create(&self) -> crate::Result<H> {
        let scope = self.inner.scope.resolve()?;
        let value = (self.inner.resolver)(scope).await?;

        self.inner.value.store(Some(Arc::new(value.clone())));

        Ok(value)
    }

    /// Returns the cached value or initializes it once among concurrent callers.
    pub async fn get_or_create(&self) -> crate::Result<H> {
        if let Some(value) = self.get() {
            return Ok(value);
        }

        let _guard = self.inner.initializing.lock().await;

        if let Some(value) = self.get() {
            return Ok(value);
        }

        self.create().await
    }
}

impl<T> Lazy<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    pub(crate) fn capture_qualified(scope: ScopeResolverSlot, qualifier: &'static str) -> Self {
        Self {
            inner: Arc::new(LazyInner {
                scope,
                resolver: Arc::new(move |scope| {
                    Box::pin(async move {
                        scope
                            .resolve_qualified::<Arc<T>>(qualifier)
                            .await?
                            .ok_or(Error::MissingComponent(short_name::<T>()))
                    })
                }),
                value: ArcSwapOption::empty(),
                initializing: AsyncMutex::new(()),
            }),
        }
    }
}

impl<H> FromContainer for Lazy<H>
where
    H: FromContainer + Clone + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        let mut dependency = H::dependency();

        dependency.resolution = ResolutionMode::Lazy;
        dependency.observation = upwell_core::DependencyObservation::Snapshot;

        dependency
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        Ok(Self::capture(cx.resolver_slot()))
    }
}

/// A dependency handle that always re-runs factory-backed targets.
pub struct Fresh<H> {
    scope: ScopeResolverSlot,
    qualifier: Option<&'static str>,
    marker: PhantomData<fn() -> H>,
}

impl<H> Clone for Fresh<H> {
    fn clone(&self) -> Self {
        Self {
            scope: self.scope.clone(),
            qualifier: self.qualifier,
            marker: PhantomData,
        }
    }
}

impl<H> Fresh<H>
where
    H: FreshFromContainer,
{
    pub(crate) fn capture(scope: ScopeResolverSlot, qualifier: Option<&'static str>) -> Self {
        Self {
            scope,
            qualifier,
            marker: PhantomData,
        }
    }

    /// Reconstructs the target without reading or replacing scoped caches.
    pub async fn create(&self) -> crate::Result<H> {
        let scope = self.scope.resolve()?;

        H::fresh_from_container(scope, self.qualifier).await
    }
}

/// A shape that can be reconstructed by [`Fresh`].
pub trait FreshFromContainer: Sized + Send + 'static {
    /// Describes the target used for build-time validation.
    fn dependency() -> DependencyDescriptor;

    /// Reconstructs this shape from factory recipes visible to `scope`.
    fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        qualifier: Option<&'static str>,
    ) -> impl Future<Output = crate::Result<Self>> + Send;
}

impl<H> FromContainer for Fresh<H>
where
    H: FreshFromContainer,
{
    fn dependency() -> DependencyDescriptor {
        let mut dependency = H::dependency();

        dependency.resolution = ResolutionMode::Fresh;
        dependency.observation = upwell_core::DependencyObservation::Snapshot;

        dependency
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        Ok(Self::capture(cx.resolver_slot(), None))
    }
}

impl<T> FreshFromContainer for Arc<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, false, false)
    }

    async fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        qualifier: Option<&'static str>,
    ) -> crate::Result<Self> {
        fresh_arc(&scope, qualifier)
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }
}

impl<H> FreshFromContainer for H
where
    H: Injectable<Target = H> + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<H>(Cardinality::One, false, false)
    }

    async fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        _: Option<&'static str>,
    ) -> crate::Result<Self> {
        let registry = scope.registry();
        let descriptor = registry
            .factory_backed(std::any::TypeId::of::<H>())
            .ok_or_else(|| Error::UnsupportedFreshFactory {
                component: short_name::<H>().into(),
                component_id: None,
                type_name: std::any::type_name::<H>().to_string(),
            })?;

        if !scope.can_access(descriptor.scope) {
            return Err(Error::MissingComponent(short_name::<H>()));
        }

        let boxed = construct_fresh_boxed(&registry, scope, descriptor).await?;

        from_boxed::<H>(&boxed).ok_or(Error::MissingComponent(short_name::<H>()))
    }
}

impl<T> FreshFromContainer for Option<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, true, false)
    }

    async fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        qualifier: Option<&'static str>,
    ) -> crate::Result<Self> {
        fresh_arc(&scope, qualifier).await
    }
}

impl<T> FreshFromContainer for Vec<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Collection, false, false)
    }

    async fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        _: Option<&'static str>,
    ) -> crate::Result<Self> {
        let registry = scope.registry();
        let mut values = fresh_construct_all::<T>(&scope).await?;

        values.sort_by_key(|(provider, _)| registry.provider_ordinal(provider));

        Ok(values.into_iter().map(|(_, value)| value).collect())
    }
}

impl<T> FreshFromContainer for HashMap<String, Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Keyed, false, false)
    }

    async fn fresh_from_container(
        scope: Arc<ScopeContainer>,
        _: Option<&'static str>,
    ) -> crate::Result<Self> {
        let values = fresh_construct_all::<T>(&scope).await?;

        Ok(values
            .into_iter()
            .map(|(provider, value)| (provider.qualifier.to_string(), value))
            .collect())
    }
}

/// Rebuilds every scope-accessible provider of trait `T` against `scope`, returning
/// each provider descriptor with its fresh value. A selected factory-less provider
/// returns [`Error::UnsupportedFreshFactory`]. Shared by the collection and keyed
/// [`Fresh`] shapes so both apply identical eligibility rules.
async fn fresh_construct_all<T>(
    scope: &Arc<ScopeContainer>,
) -> crate::Result<Vec<(ProviderDescriptor, Arc<T>)>>
where
    T: ?Sized + Send + Sync + 'static,
{
    let registry = scope.registry();
    let mut values = Vec::new();
    let providers = registry.selected_runtime_collection(
        std::any::TypeId::of::<T>(),
        ResolutionMode::Fresh,
        scope.scope(),
        &|dependency| scope.can_access(dependency),
    );

    for provider in &providers {
        let Some(descriptor) = registry.factory_backed(provider.concrete_ty.type_id) else {
            if let Some(component) = registry.component(provider.concrete_ty.type_id) {
                return Err(Error::UnsupportedFreshFactory {
                    component: component.name.to_string(),
                    component_id: Some(component.id.to_string()),
                    type_name: (component.ty.type_name)().to_string(),
                });
            }

            continue;
        };

        if !scope.can_access(descriptor.scope) {
            continue;
        }

        let concrete = construct_fresh_boxed(&registry, Arc::clone(scope), descriptor).await?;
        let erased = (provider.erase)(&concrete);
        let value =
            from_boxed::<Arc<T>>(&erased).ok_or(Error::MissingComponent(short_name::<T>()))?;

        values.push((*provider, value));
    }

    Ok(values)
}

async fn fresh_arc<T>(
    scope: &Arc<ScopeContainer>,
    qualifier: Option<&str>,
) -> crate::Result<Option<Arc<T>>>
where
    T: ?Sized + Send + Sync + 'static,
{
    let registry = scope.registry();
    let target = std::any::TypeId::of::<T>();

    if let Some(descriptor) = registry.factory_backed(target) {
        if !scope.can_access(descriptor.scope) {
            return Ok(None);
        }

        let boxed = construct_fresh_boxed(&registry, Arc::clone(scope), descriptor).await?;

        return Ok(from_boxed::<Arc<T>>(&boxed));
    }

    let provider = registry.selected_runtime_provider(
        target,
        qualifier,
        ResolutionMode::Fresh,
        scope.scope(),
        &|dependency| scope.can_access(dependency),
    );
    let Some(provider) = provider else {
        return Ok(None);
    };
    let descriptor = registry
        .factory_backed(provider.concrete_ty.type_id)
        .ok_or_else(|| Error::UnsupportedFreshFactory {
            component: provider.concrete_ty.name.into(),
            component_id: None,
            type_name: (provider.concrete_ty.type_name)().to_string(),
        })?;

    if !scope.can_access(descriptor.scope) {
        return Ok(None);
    }

    let concrete = construct_fresh_boxed(&registry, Arc::clone(scope), descriptor).await?;
    let erased = (provider.erase)(&concrete);

    Ok(from_boxed::<Arc<T>>(&erased))
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

#[cfg(test)]
mod tests;
