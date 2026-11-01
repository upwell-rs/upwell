//! Resolving components, providers and extractors from a container.

use std::any::TypeId;
use std::sync::Arc;

use upwell_core::{ResolutionMode, Scope, ScopeKey, Transient};

use super::{ScopeContainer, construct_transient, construct_transient_provider};
use crate::descriptors::component::{ComponentConstructionContext, SharedStore, from_boxed};
use crate::descriptors::{Component, Injectable, ProviderDescriptor};
use crate::layout::{DepRef, RootTarget, SlotLayout, SlotRef};

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

        if let Some(handle) = self.selected_built::<H>(target, None) {
            return Ok(Some(handle));
        }

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

            self.ensure_component(provider.concrete_ty.type_id).await?;

            return Ok(self.resolve_provider_built::<H>(&provider));
        }

        self.ensure_component(target).await?;

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

        if let Some(handle) = self.selected_built::<H>(target, Some(qualifier)) {
            return Ok(Some(handle));
        }

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

        self.ensure_component(provider.concrete_ty.type_id).await?;

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
    ///
    /// A single component already built in this scope or the scopes it opens under is read
    /// directly, without selecting providers or creating a construction context. A trait
    /// is read from the provider its scope's layout selected when the generation was
    /// prepared.
    pub async fn extract<H: crate::construct::FromContainer>(self: &Arc<Self>) -> crate::Result<H> {
        if let Some(value) = H::read_built(self) {
            return Ok(value);
        }

        self.extract_selected::<H>().await
    }

    /// Extracts `H` through runtime provider selection and a construction context, building
    /// what it resolves to where its scope's opening left it out.
    async fn extract_selected<H: crate::construct::FromContainer>(
        self: &Arc<Self>,
    ) -> crate::Result<H> {
        let dependency = H::dependency();

        if !dependency.config && dependency.resolution == ResolutionMode::Eager {
            self.ensure_for(
                dependency.ty.type_id,
                dependency.qualifier,
                dependency.cardinality,
            )
            .await?;
        }

        let cx = ComponentConstructionContext::new_with_slot(
            Transient::KEY,
            Some(Arc::clone(self)),
            Arc::clone(&self.registry),
            self.inherited_resolvers(),
            self.slot.clone(),
        );

        H::from_container(&cx).await
    }

    /// The component of type `H::Target` built in this scope or the scopes it opens under.
    /// A type nothing provides as a trait is found by its concrete type alone; one that is
    /// provided as a trait is read through [`selected_built`](Self::selected_built).
    pub(crate) fn built_component<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let mut current = self;

        if !self
            .registry
            .selection
            .providers_for_trait(type_id)
            .is_empty()
        {
            return self.selected_built::<H>(type_id, None);
        }

        loop {
            if let Some(component) = current.store.component(type_id) {
                return from_boxed::<H>(component);
            }

            current = current.parent.as_deref()?;
        }
    }

    /// The built provider of trait `target`, selected by `qualifier`, that runtime selection
    /// picks from this scope, read from the source this scope's layout selected for it.
    ///
    /// `None` sends the caller down runtime selection: when this scope was not opened from a
    /// layout, the layout selected no planned non-transient source, a scope on this chain
    /// shadows the selected provider, the source is not readable on this chain (an
    /// enclosing scope opened without that layout), or the instance is not built yet.
    ///
    /// Like planned edges, this relies on a provider's instance living only in the store of
    /// its component's scope, so the source holds the instance selection would read, and on
    /// a scope being opened with the registry of the selection its layout was planned from.
    pub(crate) fn selected_built<H: Injectable>(
        &self,
        target: TypeId,
        qualifier: Option<&str>,
    ) -> Option<H> {
        let layout = self.store.slots()?.layout();
        let selected = layout.selected(target, qualifier)?;

        if selected
            .shadowed_by
            .iter()
            .any(|scope| self.can_access(*scope))
        {
            return None;
        }

        match &selected.source {
            DepRef::Local(at) => self.planned::<H>(layout, *at),
            source => self.read_enclosing::<H>(source),
        }
    }

    /// The handle a planned source outside a scope's own slots resolves to, read from this
    /// scope's chain: an enclosing scope's slot when that scope was opened from the planned
    /// layout, or the root's singleton. `None` for any other source.
    pub(crate) fn read_enclosing<H: Injectable>(&self, source: &DepRef) -> Option<H> {
        match source {
            DepRef::Ancestor { layout, at } => {
                self.ancestor(layout.scope())?.planned::<H>(layout, *at)
            }
            DepRef::Root(target) => self.root_container().root_target::<H>(target),
            DepRef::Local(_)
            | DepRef::Transient(_)
            | DepRef::Many(_)
            | DepRef::Absent
            | DepRef::Unplanned => None,
        }
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
