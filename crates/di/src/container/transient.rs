//! Constructing transient and `Fresh` instances against a requesting scope.

use std::any::TypeId;
use std::sync::Arc;

use upwell_core::Transient;

use super::{ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::descriptors::component::{ComponentConstructionContext, SharedStore};
use crate::descriptors::{BoxedComponent, ComponentDescriptor, Injectable, ProviderDescriptor};
use crate::error::Error;

/// Constructs a fresh `Transient` instance whose `Injectable::Target` is `H`, if
/// one is registered. A transient is rebuilt on every resolution and never stored,
/// so it is constructed in a throwaway context parented to `parent` (its
/// dependencies — singletons in v1 — resolve up the chain). `slot` is the capture
/// slot of the scope on whose behalf the transient is being built, so its
/// `Deferred`/`Lazy`/`Fresh` fields hydrate with that scope, and `store` is the
/// in-progress store of that scope, so its eager dependencies resolve from
/// components already built there.
async fn construct_transient_boxed(
    registry: &Arc<ScopeRegistry>,
    parent: Option<Arc<ScopeContainer>>,
    slot: ScopeResolverSlot,
    store: SharedStore,
    descriptor: ComponentDescriptor,
) -> crate::Result<BoxedComponent> {
    let factory = descriptor
        .effective_factory()?
        .ok_or(Error::MissingComponent(descriptor.name))?;

    let externals = parent
        .as_ref()
        .map(|p| p.resolvers().clone())
        .unwrap_or_default();

    let mut cx = ComponentConstructionContext::with_store(
        &Transient,
        parent,
        Arc::clone(registry),
        externals,
        slot,
        store,
    );

    let boxed = (factory.construct)(&mut cx).await?;

    Ok(boxed)
}

pub(crate) async fn construct_fresh_boxed(
    registry: &Arc<ScopeRegistry>,
    owner: Arc<ScopeContainer>,
    descriptor: ComponentDescriptor,
) -> crate::Result<BoxedComponent> {
    let factory =
        descriptor
            .effective_factory()?
            .ok_or_else(|| Error::UnsupportedFreshFactory {
                component: descriptor.name.to_string(),
                component_id: Some(descriptor.id.to_string()),
                type_name: (descriptor.ty.type_name)().to_string(),
            })?;
    let target_scope = if descriptor.scope.is_transient() {
        // A transient has no container of its own: rebuild it in a throwaway
        // context parented to the requesting scope, like on-demand transient
        // construction, so its dependencies resolve up the owner's chain.
        Arc::clone(&owner)
    } else {
        owner
            .container_for_scope(descriptor.scope)
            .ok_or(Error::MissingComponent(descriptor.name))?
    };
    let externals = target_scope.resolvers().clone();
    let slot = owner.slot.clone();
    let mut cx = ComponentConstructionContext::new_with_slot(
        descriptor.scope,
        Some(target_scope),
        Arc::clone(registry),
        externals,
        slot,
    );

    (factory.construct)(&mut cx).await
}

pub(crate) async fn construct_transient<H: Injectable>(
    registry: &Arc<ScopeRegistry>,
    parent: Option<Arc<ScopeContainer>>,
    slot: ScopeResolverSlot,
    store: SharedStore,
) -> crate::Result<Option<H>> {
    let target = TypeId::of::<H::Target>();
    let Some(descriptor) = registry.transient(target) else {
        return Ok(None);
    };
    let boxed = construct_transient_boxed(registry, parent, slot, store, descriptor).await?;

    Ok(crate::descriptors::component::from_boxed::<H>(&boxed))
}

pub(crate) async fn construct_transient_provider<H: Injectable>(
    registry: &Arc<ScopeRegistry>,
    parent: Option<Arc<ScopeContainer>>,
    slot: ScopeResolverSlot,
    store: SharedStore,
    provider: ProviderDescriptor,
) -> crate::Result<Option<H>> {
    let Some(descriptor) = registry.transient(provider.concrete_ty.type_id) else {
        return Ok(None);
    };
    let concrete = construct_transient_boxed(registry, parent, slot, store, descriptor).await?;
    let erased = (provider.erase)(&concrete);

    Ok(crate::descriptors::component::from_boxed::<H>(&erased))
}
