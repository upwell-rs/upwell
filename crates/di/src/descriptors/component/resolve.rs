//! Resolving a factory's dependencies, by declared position or by type.
//!
//! A generated factory reads each edge by the position it declared it at, so a planned slot
//! answers with one index. A hand-written factory resolves by type, which searches the
//! running slot's planned edges. Either falls back to dynamic selection when nothing was
//! planned. The `*_ready` reads answer a factory's synchronous path from planned edges
//! alone, and report an edge that needs more so the factory runs its async path.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, ResolutionMode, ScopeKey};

use super::planned::Planned;
use super::{ComponentConstructionContext, Injectable, ProviderDescriptor, ScopeStore};

impl ComponentConstructionContext {
    /// Resolves a single dependency by its injectable handle `H`, keyed under
    /// `H::Target`: this scope first, then each longer-lived parent scope, then —
    /// if `H::Target` is a `Transient` component — a freshly constructed instance.
    pub async fn resolve<H: Injectable>(&self) -> crate::Result<Option<H>> {
        self.resolve_one::<H>(None, None).await
    }

    /// Qualifier-selected single provider, this scope then parents.
    pub async fn resolve_qualified<H: Injectable>(
        &self,
        qualifier: &str,
    ) -> crate::Result<Option<H>> {
        self.resolve_one::<H>(None, Some(qualifier)).await
    }

    /// Every provider of the trait `H::Target` across this scope and its parents.
    pub async fn resolve_all<H: Injectable>(&self) -> crate::Result<Vec<H>> {
        self.resolve_collection::<H>(None).await
    }

    /// Every provider of the trait `H::Target` keyed by qualifier, across this scope
    /// and its parents (a closer scope wins a qualifier collision).
    pub async fn resolve_keyed<H: Injectable>(&self) -> crate::Result<HashMap<String, H>> {
        self.resolve_keyed_at::<H>(None).await
    }

    /// Resolves the single edge a generated factory declared at `index`, selected by
    /// `qualifier` when it has one. Behaves as [`resolve`](Self::resolve) or
    /// [`resolve_qualified`](Self::resolve_qualified) when the running factory planned
    /// no matching edge there.
    #[doc(hidden)]
    pub async fn dep<H: Injectable>(
        &self,
        index: usize,
        qualifier: Option<&str>,
    ) -> crate::Result<Option<H>> {
        self.resolve_one::<H>(Some(index), qualifier).await
    }

    /// Resolves the collection edge a generated factory declared at `index`. Behaves as
    /// [`resolve_all`](Self::resolve_all) when no matching edge was planned there.
    #[doc(hidden)]
    pub async fn dep_all<H: Injectable>(&self, index: usize) -> crate::Result<Vec<H>> {
        self.resolve_collection::<H>(Some(index)).await
    }

    /// Resolves the keyed edge a generated factory declared at `index`. Behaves as
    /// [`resolve_keyed`](Self::resolve_keyed) when no matching edge was planned there.
    #[doc(hidden)]
    pub async fn dep_keyed<H: Injectable>(
        &self,
        index: usize,
    ) -> crate::Result<HashMap<String, H>> {
        self.resolve_keyed_at::<H>(Some(index)).await
    }

    /// Reads the single edge a generated factory declared at `index` without awaiting.
    /// `None` when its planned source needs construction or dynamic selection; otherwise
    /// the handle, or `Some(None)` when the plan found no provider.
    #[doc(hidden)]
    pub fn dep_ready<H: Injectable>(
        &self,
        index: usize,
        qualifier: Option<&str>,
    ) -> Option<Option<H>> {
        let target = TypeId::of::<H::Target>();
        let source = self.planned_edge(Some(index), target, qualifier, Cardinality::One)?;

        match self.read_planned::<H>(source) {
            Planned::Resolved(handle) => Some(handle),
            Planned::Unavailable => None,
        }
    }

    /// Reads the collection edge a generated factory declared at `index` without awaiting.
    /// `None` when one of its planned providers needs construction or dynamic selection.
    #[doc(hidden)]
    pub fn dep_all_ready<H: Injectable>(&self, index: usize) -> Option<Vec<H>> {
        let target = TypeId::of::<H::Target>();
        let provided = self.read_many::<H>(index, target, Cardinality::Collection)?;

        Some(provided.into_iter().map(|(_, handle)| handle).collect())
    }

    /// Reads the keyed edge a generated factory declared at `index` without awaiting.
    /// `None` when one of its planned providers needs construction or dynamic selection.
    #[doc(hidden)]
    pub fn dep_keyed_ready<H: Injectable>(&self, index: usize) -> Option<HashMap<String, H>> {
        let target = TypeId::of::<H::Target>();
        let provided = self.read_many::<H>(index, target, Cardinality::Keyed)?;

        Some(
            provided
                .into_iter()
                .map(|(qualifier, handle)| (qualifier.to_string(), handle))
                .collect(),
        )
    }

    async fn resolve_one<H: Injectable>(
        &self,
        index: Option<usize>,
        qualifier: Option<&str>,
    ) -> crate::Result<Option<H>> {
        let target = TypeId::of::<H::Target>();

        if let Some(source) = self.planned_edge(index, target, qualifier, Cardinality::One)
            && let Planned::Resolved(handle) = self.resolve_planned::<H>(source).await?
        {
            return Ok(handle);
        }

        let selected = self.registry.selected_runtime_provider(
            target,
            qualifier,
            ResolutionMode::Eager,
            self.scope,
            &|scope| self.can_access(scope),
        );

        if let Some(provider) = selected {
            return self.resolve_selected::<H>(provider).await;
        }

        if qualifier.is_some() {
            return Ok(None);
        }

        if let Some(handle) = self.read_store(ScopeStore::resolve_local::<H>) {
            return Ok(Some(handle));
        }

        if let Some(parent) = &self.parent
            && let Some(handle) = parent.resolve_built::<H>()
        {
            return Ok(Some(handle));
        }

        crate::container::construct_transient::<H>(
            &self.registry,
            self.parent.clone(),
            self.slot.clone(),
            self.store_handle(),
        )
        .await
    }

    async fn resolve_collection<H: Injectable>(
        &self,
        index: Option<usize>,
    ) -> crate::Result<Vec<H>> {
        let target = TypeId::of::<H::Target>();

        if let Some(provided) = self
            .planned_many::<H>(index, target, Cardinality::Collection)
            .await?
        {
            return Ok(provided.into_iter().map(|(_, handle)| handle).collect());
        }

        let providers = self.registry.selected_runtime_collection(
            target,
            ResolutionMode::Eager,
            self.scope,
            |scope| self.can_access(scope),
        );
        let mut all = Vec::with_capacity(providers.size_hint().1.unwrap_or_default());

        for provider in providers {
            if let Some(value) = self.resolve_selected::<H>(provider).await? {
                all.push(value);
            }
        }

        Ok(all)
    }

    async fn resolve_keyed_at<H: Injectable>(
        &self,
        index: Option<usize>,
    ) -> crate::Result<HashMap<String, H>> {
        let target = TypeId::of::<H::Target>();

        if let Some(provided) = self
            .planned_many::<H>(index, target, Cardinality::Keyed)
            .await?
        {
            return Ok(provided
                .into_iter()
                .map(|(qualifier, handle)| (qualifier.to_string(), handle))
                .collect());
        }

        let providers = self.registry.selected_runtime_keyed(
            target,
            ResolutionMode::Eager,
            self.scope,
            &|scope| self.can_access(scope),
        );
        let mut keyed = HashMap::with_capacity(providers.len());

        for provider in providers {
            if let Some(value) = self.resolve_selected::<H>(provider).await? {
                keyed.insert(provider.qualifier.to_string(), value);
            }
        }

        Ok(keyed)
    }

    /// Whether a component in `scope` is visible from this construction scope.
    fn can_access(&self, scope: ScopeKey) -> bool {
        scope.is_transient()
            || scope.id() == self.scope.id()
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.can_access(scope))
    }

    /// Resolves one selected provider: a transient provider is constructed fresh,
    /// anything else is read from this scope's store or its parents. Checking for a
    /// transient first keeps the common built case free of the construction setup.
    async fn resolve_selected<H: Injectable>(
        &self,
        provider: ProviderDescriptor,
    ) -> crate::Result<Option<H>> {
        if self
            .registry
            .transient(provider.concrete_ty.type_id)
            .is_some()
        {
            return crate::container::construct_transient_provider::<H>(
                &self.registry,
                self.parent.clone(),
                self.slot.clone(),
                self.store_handle(),
                provider,
            )
            .await;
        }

        if let Some(handle) = self.read_store(|store| store.resolve_provider_local::<H>(&provider))
        {
            return Ok(Some(handle));
        }

        Ok(self
            .parent
            .as_ref()
            .and_then(|parent| parent.resolve_provider_built::<H>(&provider)))
    }
}
