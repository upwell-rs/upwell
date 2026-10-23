//! `Lazy<H>`: a dependency resolved on first use from the scope that captured it.

use std::{future::Future, pin::Pin, sync::Arc};

use arc_swap::ArcSwapOption;
use tokio::sync::Mutex as AsyncMutex;
use upwell_core::{DependencyDescriptor, ResolutionMode};

use crate::{
    ComponentConstructionContext, FromContainer, ScopeContainer, construct::short_name,
    container::ScopeResolverSlot, error::Error,
};

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
