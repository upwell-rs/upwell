//! Resolving components, providers and extractors from a container.

use std::any::TypeId;
use std::sync::Arc;

use upwell_core::{ResolutionMode, Scope, ScopeKey, Transient};

use super::{ScopeContainer, construct_transient, construct_transient_provider};
use crate::descriptors::component::{ComponentConstructionContext, SharedStore};
use crate::descriptors::{Component, Injectable, ProviderDescriptor};
use crate::layout::{RootTarget, SlotLayout, SlotRef};

impl ScopeContainer {
    /// Returns the registered component of type `T` as its handle (`Arc<T>` by
    /// default, or the by-value handle for a `#[component(by_value)]` type),
    /// resolved through this scope and its parents.
    pub fn get<T: Component>(&self) -> Option<T::Handle> {
        self.resolve_built::<T::Handle>()
    }

    /// Resolves `H` through this scope then each parent, or — if `H::Target` is a
    /// `Transient` — constructs a fresh instance.
    pub async fn resolve<H: Injectable>(self: &Arc<Self>) -> crate::Result<Option<H>> {
        let target = TypeId::of::<H::Target>();

        if let Some(provider) = self.registry.selected_runtime_provider(
            target,
            None,
            ResolutionMode::Eager,
            self.scope,
            &|scope| self.can_access(scope),
        ) {
            if self
                .registry
                .transient(provider.concrete_ty.type_id)
                .is_some()
            {
                return construct_transient_provider::<H>(
                    &self.registry,
                    Some(Arc::clone(self)),
                    self.slot.clone(),
                    SharedStore::default(),
                    provider,
                )
                .await;
            }

            return Ok(self.resolve_provider_built::<H>(&provider));
        }

        if let Some(handle) = self.resolve_built::<H>() {
            return Ok(Some(handle));
        }

        construct_transient::<H>(
            &self.registry,
            Some(Arc::clone(self)),
            self.slot.clone(),
            SharedStore::default(),
        )
        .await
    }

    /// Resolves a qualifier-selected trait provider from this scope.
    #[doc(hidden)]
    pub async fn resolve_qualified<H: Injectable>(
        self: &Arc<Self>,
        qualifier: &str,
    ) -> crate::Result<Option<H>> {
        let target = TypeId::of::<H::Target>();
        let Some(provider) = self.registry.selected_runtime_provider(
            target,
            Some(qualifier),
            ResolutionMode::Eager,
            self.scope,
            &|scope| self.can_access(scope),
        ) else {
            return Ok(None);
        };

        if self
            .registry
            .transient(provider.concrete_ty.type_id)
            .is_some()
        {
            return construct_transient_provider::<H>(
                &self.registry,
                Some(Arc::clone(self)),
                self.slot.clone(),
                SharedStore::default(),
                provider,
            )
            .await;
        }

        Ok(self.resolve_provider_built::<H>(&provider))
    }

    /// Extracts any [`FromContainer`](crate::FromContainer) value from this scope — the
    /// request-time analogue of a factory parameter.
    ///
    /// Views the scope as a construction context parented at itself, so the *whole*
    /// `FromContainer` set resolves the same way a constructor's parameters do: components
    /// (`Arc<T>`, `Dep<T>`, by-value injectables, and `Vec`/`HashMap`/`Option` of providers)
    /// through the scope chain, and resolver-backed values (`Cfg<T>`, and any future
    /// resolver-sourced type) through the threaded resolvers. A protocol's `Inject` extractor
    /// builds on this, so a handler can inject anything a constructor can — not just the
    /// `Injectable` subset that [`resolve`](Self::resolve) covers.
    pub async fn extract<H: crate::construct::FromContainer>(self: &Arc<Self>) -> crate::Result<H> {
        let cx = ComponentConstructionContext::new_with_slot(
            Transient::KEY,
            Some(Arc::clone(self)),
            Arc::clone(&self.registry),
            self.resolvers().clone(),
            self.slot.clone(),
        );

        H::from_container(&cx).await
    }

    /// Single concrete-or-primary-provider lookup across this scope and its parents.
    pub(crate) fn resolve_built<H: Injectable>(&self) -> Option<H> {
        if let Some(handle) = self.store.resolve_local::<H>() {
            return Some(handle);
        }

        self.parent.as_ref()?.resolve_built::<H>()
    }

    /// Qualifier-selected single provider across this scope and its parents.
    pub(crate) fn resolve_qualified_built<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        if let Some(handle) = self.store.resolve_qualified_local::<H>(qualifier) {
            return Some(handle);
        }

        self.parent
            .as_ref()?
            .resolve_qualified_built::<H>(qualifier)
    }

    /// The container of `scope` on this scope's chain, this scope included.
    pub(crate) fn ancestor(&self, scope: ScopeKey) -> Option<&ScopeContainer> {
        let mut current = self;

        loop {
            if current.scope.id() == scope.id() {
                return Some(current);
            }

            current = current.parent.as_deref()?;
        }
    }

    /// The root container at the top of this scope's chain.
    pub(crate) fn root_container(&self) -> &ScopeContainer {
        let mut current = self;

        while let Some(parent) = current.parent.as_deref() {
            current = parent;
        }

        current
    }

    /// The handle at a planned slot of this scope, when it was opened from `layout`.
    pub(crate) fn planned<H: Injectable>(&self, layout: &SlotLayout, at: SlotRef) -> Option<H> {
        self.store.planned::<H>(layout, at)
    }

    /// The handle a planned root dependency resolves to in this container.
    pub(crate) fn root_target<H: Injectable>(&self, target: &RootTarget) -> Option<H> {
        self.store.root::<H>(target)
    }

    /// Resolves one exact selected provider from its owning visible scope.
    pub(crate) fn resolve_provider_built<H: Injectable>(
        &self,
        provider: &ProviderDescriptor,
    ) -> Option<H> {
        if let Some(handle) = self.store.resolve_provider_local::<H>(provider) {
            return Some(handle);
        }

        self.parent.as_ref()?.resolve_provider_built::<H>(provider)
    }
}
