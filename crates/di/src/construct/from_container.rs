//! [`FromContainer`]: the per-parameter extractor factories resolve their arguments with.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use upwell_core::{Cardinality, DependencyDescriptor, DependencyObservation};

use super::{dependency_of, dependency_of_observed, short_name};
use crate::descriptors::{ComponentConstructionContext, Dep, Injectable};
use crate::error::Error;

/// A single factory parameter, resolvable from the construction context, that
/// reports the dependency edge it represents.
///
/// Implemented for the injectable parameter shapes — `Arc<T>` (a single component
/// or the primary/sole `Arc<dyn Trait>`), a by-value injectable (`Dir<K>`,
/// `PeerInfo`, a `#[component(by_value)]` type), `Option<Arc<T>>` (an optional
/// component), `Vec<Arc<dyn Trait>>` (every provider), and
/// `HashMap<String, Arc<dyn Trait>>` (providers keyed by qualifier). The config layer
/// adds an impl for `Cfg<T>` in its own crate (resolving via the config resolver).
pub trait FromContainer: Sized {
    /// The dependency edge this parameter contributes, for validation and ordering.
    fn dependency() -> DependencyDescriptor;

    /// Resolves this parameter from the construction context.
    fn from_container(
        cx: &ComponentConstructionContext,
    ) -> impl Future<Output = crate::Result<Self>> + Send;

    /// Resolves this parameter as the factory's edge declared at `index`, reading the
    /// running slot's planned edge there directly. Defaults to
    /// [`from_container`](Self::from_container) for parameters that resolve through their
    /// own handle.
    fn from_container_at(
        cx: &ComponentConstructionContext,
        _index: usize,
    ) -> impl Future<Output = crate::Result<Self>> + Send {
        Self::from_container(cx)
    }

    /// Resolves this parameter as the edge declared at `index` without awaiting. `None`
    /// means the edge needs construction or dynamic selection, so the factory takes its
    /// async path; parameters without a synchronous read always return `None`.
    fn ready_at(_cx: &ComponentConstructionContext, _index: usize) -> Option<crate::Result<Self>> {
        None
    }
}

impl<T> FromContainer for Arc<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Arc<T>>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep::<Arc<T>>(index, None)
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        required::<Arc<T>, T>(cx, index)
    }
}

/// A live, reloadable dependency. Resolves the same component as `Arc<T>` (keyed by
/// `T`), but hands back a `Dep<T>` sharing the component's live slot, so a later
/// reload swap is observed. `Target = T ≠ Dep<T>`, so this does not overlap the
/// by-value blanket below.
impl<T> FromContainer for Dep<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of_observed::<T>(Cardinality::One, false, false, DependencyObservation::Live)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Dep<T>>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep::<Dep<T>>(index, None)
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        required::<Dep<T>, T>(cx, index)
    }
}

/// By-value injectables — those whose injectable handle *is* the type itself
/// (`Injectable<Target = Self>`): `Dir<K>`, `PeerInfo`, `ShutdownHandle`, and any
/// `#[component(by_value)]` type. The `Target = Self` bound excludes `Arc<T>`
/// (`Target = T`) and `Cfg<T>` (`Target = T`), so this does not overlap their
/// dedicated impls.
impl<H> FromContainer for H
where
    H: Injectable<Target = H> + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<H>(Cardinality::One, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<H>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<H>()))
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep::<H>(index, None)
            .await?
            .ok_or(Error::MissingComponent(short_name::<H>()))
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        required::<H, H>(cx, index)
    }
}

impl<T> FromContainer for Option<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, true, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Arc<T>>().await
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep::<Arc<T>>(index, None).await
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        cx.dep_ready::<Arc<T>>(index, None).map(Ok)
    }
}

impl<T> FromContainer for Vec<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Collection, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve_all::<Arc<T>>().await
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep_all::<Arc<T>>(index).await
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        cx.dep_all_ready::<Arc<T>>(index).map(Ok)
    }
}

impl<T> FromContainer for HashMap<String, Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Keyed, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve_keyed::<Arc<T>>().await
    }

    async fn from_container_at(
        cx: &ComponentConstructionContext,
        index: usize,
    ) -> crate::Result<Self> {
        cx.dep_keyed::<Arc<T>>(index).await
    }

    fn ready_at(cx: &ComponentConstructionContext, index: usize) -> Option<crate::Result<Self>> {
        cx.dep_keyed_ready::<Arc<T>>(index).map(Ok)
    }
}

/// Reads the required single edge declared at `index` as handle `H`, naming `T` when no
/// provider was planned for it.
fn required<H: Injectable, T: ?Sized + 'static>(
    cx: &ComponentConstructionContext,
    index: usize,
) -> Option<crate::Result<H>> {
    let handle = cx.dep_ready::<H>(index, None)?;

    Some(handle.ok_or(Error::MissingComponent(short_name::<T>())))
}
