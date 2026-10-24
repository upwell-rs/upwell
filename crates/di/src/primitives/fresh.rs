//! `Fresh<H>`: a dependency constructed anew on every request.

use std::{collections::HashMap, future::Future, marker::PhantomData, sync::Arc};

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode};

use crate::{
    ComponentConstructionContext, FromContainer, Injectable, ProviderDescriptor, ScopeContainer,
    construct::{dependency_of, short_name},
    container::{ScopeResolverSlot, construct_fresh_boxed},
    descriptors::component::from_boxed,
    error::Error,
};

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
pub(super) async fn fresh_construct_all<T>(
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
        |dependency| scope.can_access(dependency),
    );

    for provider in providers {
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

        values.push((provider, value));
    }

    Ok(values)
}

pub(super) async fn fresh_arc<T>(
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
