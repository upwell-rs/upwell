//! Building the root container and opening child scopes.

use std::any::{Any, TypeId};
use std::sync::Arc;

use tracing::{debug, error, info, instrument, trace};
use upwell_core::{ResolverSet, Scope, ScopeKey, Singleton};

use super::{ComponentSource, ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::descriptors::component::{
    ComponentConstructionContext, ComponentConstructionParts, Running, ScopeStore,
};
use crate::descriptors::{BoxedComponent, ComponentDescriptor};
use crate::error::Error;
use crate::layout::{SlotIdx, SlotLayout};

/// The runtime-generation state object a container retains while it is open.
type GenerationLease = Option<Arc<dyn Any + Send + Sync>>;

impl ScopeContainer {
    /// Returns the container whose identity matches `scope` from this scope's
    /// ancestry. Fresh construction uses this boundary as its resolution root so
    /// the rebuilt target cannot observe components from its shorter-lived owner.
    pub(super) fn container_for_scope(
        self: &Arc<Self>,
        scope: ScopeKey,
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
            Singleton::KEY,
            None,
            registry,
            order,
            instances,
            externals,
            None,
        )
        .await?;

        info!(count = root.store.component_count(), "root container built");

        Ok(root)
    }

    /// Opens a child scope over `parent`, seeding `seeds` then constructing `order`
    /// (a dependency order precomputed at application build).
    ///
    /// Every dependency is selected while the scope is built. Runtime-managed scopes are
    /// opened from a prepared layout with [`open_planned`](Self::open_planned) instead.
    /// Empty scopes still receive a container so every declared logical boundary
    /// retains its stable identity in the runtime chain.
    pub async fn open_child(
        scope: ScopeKey,
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let generation_lease = parent.generation_lease.clone();

        if order.is_empty() && seeds.is_empty() {
            return Ok(Self::empty(scope, parent, registry, generation_lease));
        }

        let externals = parent.resolvers().clone();

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

    /// Opens a child scope from its prepared slot layout, retaining one opaque
    /// runtime-generation state object.
    ///
    /// Seeds fill the layout's factory-less slots, then each factory-backed slot is
    /// built in layout order, resolving its dependencies through the edges the layout
    /// planned. The layout must belong to the generation of `registry`. A seed slot left
    /// empty fails only when a dependency resolves it, as with a scope opened from a plain
    /// construction order.
    #[doc(hidden)]
    pub async fn open_planned<T: Any + Send + Sync>(
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        layout: &Arc<SlotLayout>,
        seeds: Vec<BoxedComponent>,
        generation_lease: Arc<T>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let scope = layout.scope();
        let generation_lease: GenerationLease = Some(generation_lease);

        if !layout.constructs() && seeds.is_empty() {
            return Ok(Self::empty(scope, parent, registry, generation_lease));
        }

        let mut cx = ComponentConstructionContext::new_with_slot(
            scope,
            Some(Arc::clone(&parent)),
            Arc::clone(&registry),
            parent.resolvers().clone(),
            ScopeResolverSlot::default(),
        );

        cx.use_layout(Arc::clone(layout));

        for seed in seeds {
            cx.insert(seed)?;
        }

        for (index, spec) in layout.slots().iter().enumerate() {
            let component = &spec.component;
            let Some(factory) = spec.factory else {
                continue;
            };

            debug!(component = %component.name, scope = scope.name(), "constructing component");

            cx.set_running(Running::Slot(SlotIdx::new(index)));

            let built = (factory.construct)(&mut cx).await;

            cx.set_running(Running::Unplanned);
            cx.insert(built?)?;

            trace!(component = %component.name, "component ready");
        }

        Self::freeze(cx.into_parts(), generation_lease)
    }

    /// A container for a scope that holds no components.
    fn empty(
        scope: ScopeKey,
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        generation_lease: GenerationLease,
    ) -> Arc<ScopeContainer> {
        let externals = parent.resolvers().clone();

        Arc::new_cyclic(|container| ScopeContainer {
            scope,
            store: ScopeStore::default(),
            parent: Some(parent),
            registry,
            resolver_base: externals,
            resolvers: std::sync::OnceLock::new(),
            slot: ScopeResolverSlot::attached(container.clone()),
            generation_lease,
            drop_guard: std::sync::OnceLock::new(),
        })
    }

    /// Seeds instances, then constructs `order` in sequence, aliasing trait
    /// providers as each instance lands. Resolution during construction reaches the
    /// parent chain.
    async fn build(
        scope: ScopeKey,
        parent: Option<Arc<ScopeContainer>>,
        registry: Arc<ScopeRegistry>,
        order: &[ComponentDescriptor],
        seeds: Vec<BoxedComponent>,
        externals: ResolverSet,
        generation_lease: GenerationLease,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let mut cx = ComponentConstructionContext::new_with_slot(
            scope,
            parent,
            Arc::clone(&registry),
            externals,
            ScopeResolverSlot::default(),
        );

        for seed in seeds {
            let type_id = seed.ty.type_id;

            cx.insert(seed)?;
            register_providers_for(&mut cx, &registry, type_id);
        }

        for descriptor in order {
            match descriptor.effective_factory()? {
                Some(factory) => {
                    debug!(component = %descriptor.name, scope = scope.name(), "constructing component");

                    let component = (factory.construct)(&mut cx).await?;

                    cx.insert(component)?;

                    trace!(component = %descriptor.name, "component ready");
                }

                None => require_seeded(&cx, descriptor)?,
            }

            register_providers_for(&mut cx, &registry, descriptor.ty.type_id);
        }

        Self::freeze(cx.into_parts(), generation_lease)
    }

    /// Freezes a built scope. The container is built with [`Arc::new_cyclic`] so it can
    /// hold its own [`ComponentSource`] (a `Weak` self-reference) in its resolver set.
    fn freeze(
        parts: ComponentConstructionParts,
        generation_lease: GenerationLease,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let slot = parts.slot;
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

/// Requires a factory-less component to have been seeded before the scope is built.
fn require_seeded(
    cx: &ComponentConstructionContext,
    descriptor: &ComponentDescriptor,
) -> crate::Result<()> {
    if !cx.contains(descriptor.ty.type_id) {
        error!(component = %descriptor.name, "no instance provided for factory-less component");

        return Err(Error::MissingComponent(descriptor.name));
    }

    trace!(component = %descriptor.name, "using provided instance");

    Ok(())
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
