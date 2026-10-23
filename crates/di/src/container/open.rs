//! Building the root container and opening child scopes.

use std::any::{Any, TypeId};
use std::sync::Arc;

use tracing::{debug, error, info, instrument, trace};
use upwell_core::{ResolverSet, Scope, Singleton};

use super::{ComponentSource, ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::descriptors::component::{ComponentConstructionContext, ScopeStore};
use crate::descriptors::{BoxedComponent, ComponentDescriptor};
use crate::error::Error;

impl ScopeContainer {
    /// Returns the container whose identity matches `scope` from this scope's
    /// ancestry. Fresh construction uses this boundary as its resolution root so
    /// the rebuilt target cannot observe components from its shorter-lived owner.
    pub(super) fn container_for_scope(
        self: &Arc<Self>,
        scope: &'static dyn Scope,
    ) -> Option<Arc<ScopeContainer>> {
        if self.scope.id() == scope.id() {
            return Some(Arc::clone(self));
        }

        self.parent
            .as_ref()
            .and_then(|parent| parent.container_for_scope(scope))
    }

    /// Resolves a retained singleton construction plan into the root container.
    ///
    /// `order` is the validated singleton construction order. `instances` holds
    /// pre-built singletons supplied at the builder;
    /// they are seeded first, so factory-built components may depend on them.
    /// `externals` is the external resolver set (e.g. the config store) the factories
    /// resolve `Cfg<T>` and similar through.
    #[instrument(skip_all, fields(count = order.len()))]
    pub async fn build_root(
        order: &[ComponentDescriptor],
        instances: Vec<BoxedComponent>,
        externals: ResolverSet,
        registry: Arc<ScopeRegistry>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let root = Self::build(
            &Singleton, None, registry, order, instances, externals, None,
        )
        .await?;

        info!(count = root.store.components.len(), "root container built");

        Ok(root)
    }

    /// Opens a child scope over `parent`, seeding `seeds` then constructing `order`
    /// (a dependency order precomputed at application build). Every scope — the four
    /// built-ins and any future user-defined one — is created through this single
    /// primitive.
    ///
    /// Empty scopes still receive a container so every declared logical boundary
    /// retains its stable identity in the runtime chain.
    pub async fn open_child(
        scope: &'static dyn Scope,
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let generation_lease = parent.generation_lease.clone();

        Self::open_child_inner(scope, parent, registry, order, seeds, generation_lease).await
    }

    /// Opens a child scope while retaining one opaque runtime-generation state object.
    #[doc(hidden)]
    pub async fn open_child_with_generation_lease<T: Any + Send + Sync>(
        scope: &'static dyn Scope,
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
        generation_lease: Arc<T>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        Self::open_child_inner(
            scope,
            parent,
            registry,
            order,
            seeds,
            Some(generation_lease),
        )
        .await
    }

    async fn open_child_inner(
        scope: &'static dyn Scope,
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
        generation_lease: Option<Arc<dyn Any + Send + Sync>>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let externals = parent.resolvers().clone();

        if order.is_empty() && seeds.is_empty() {
            return Ok(Arc::new_cyclic(|container| ScopeContainer {
                scope,
                store: ScopeStore::default(),
                parent: Some(parent),
                registry,
                resolver_base: externals,
                resolvers: std::sync::OnceLock::new(),
                slot: ScopeResolverSlot::attached(container.clone()),
                generation_lease,
                drop_guard: std::sync::OnceLock::new(),
            }));
        }

        Self::build(
            scope,
            Some(parent),
            registry,
            order,
            seeds,
            externals,
            generation_lease,
        )
        .await
    }

    /// Seeds instances, then constructs `order` in sequence, aliasing trait
    /// providers as each instance lands. Resolution during construction reaches the
    /// parent chain. The frozen container is built with [`Arc::new_cyclic`] so it can
    /// hold its own [`ComponentSource`] (a `Weak` self-reference) in its resolver set.
    pub(super) async fn build(
        scope: &'static dyn Scope,
        parent: Option<Arc<ScopeContainer>>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
        externals: ResolverSet,
        generation_lease: Option<Arc<dyn Any + Send + Sync>>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let slot = ScopeResolverSlot::default();
        let mut cx = ComponentConstructionContext::new_with_slot(
            scope,
            parent,
            Arc::clone(&registry),
            externals,
            slot.clone(),
        );
        for seed in seeds {
            let type_id = seed.ty.type_id;

            cx.insert(seed);
            register_providers_for(&mut cx, &registry, type_id);
        }

        for descriptor in order {
            match descriptor.effective_factory()? {
                Some(factory) => {
                    debug!(component = %descriptor.name, scope = scope.name(), "constructing component");

                    let component = (factory.construct)(&mut cx).await?;

                    cx.insert(component);

                    trace!(component = %descriptor.name, "component ready");
                }

                None => {
                    if !cx.contains(descriptor.ty.type_id) {
                        error!(component = %descriptor.name, "no instance provided for factory-less component");
                        return Err(Error::MissingComponent(descriptor.name));
                    }

                    trace!(component = %descriptor.name, "using provided instance");
                }
            }

            register_providers_for(&mut cx, &registry, descriptor.ty.type_id);
        }

        let parts = cx.into_parts();
        let slot = parts.slot.clone();

        let container = Arc::new_cyclic(|weak| {
            let mut resolvers = parts.resolvers;
            resolvers.insert(Arc::new(ComponentSource {
                container: weak.clone(),
            }));

            ScopeContainer {
                scope: parts.scope,
                store: parts.store,
                parent: parts.parent,
                registry: parts.registry,
                resolver_base: resolvers.clone(),
                resolvers: std::sync::OnceLock::from(resolvers),
                slot: slot.clone(),
                generation_lease,
                drop_guard: std::sync::OnceLock::new(),
            }
        });
        slot.attach(&container)?;

        Ok(container)
    }
}

/// Registers every provider declared by the just-built concrete `concrete_id`,
/// aliasing its single instance under each trait it provides.
fn register_providers_for(
    cx: &mut ComponentConstructionContext,
    registry: &ScopeRegistry,
    concrete_id: TypeId,
) {
    for provider in registry.providers_for(concrete_id) {
        let ordinal = registry.provider_ordinal(provider);
        cx.register_provider(provider, ordinal);
    }
}
