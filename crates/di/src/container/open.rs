//! Building the root container and opening child scopes.

use std::any::{Any, TypeId};
use std::sync::Arc;

use tracing::{debug, error, info, instrument, trace};
use upwell_core::{ResolverSet, Scope, ScopeKey, Singleton};

use super::{ComponentSource, ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::descriptors::component::{
    ComponentConstructionContext, ComponentConstructionParts, HandleStorage, Running, ScopeStore,
};
use crate::descriptors::{BoxedComponent, ComponentDescriptor};
use crate::error::Error;
use crate::layout::{EntryPlan, SlotIdx, SlotLayout};

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

        let externals = parent.inherited_resolvers();

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
    /// construction order. A seed without a factory-less slot in the layout, or a second seed
    /// for the same slot, fails the opening.
    ///
    /// With an entry `plan`, only the slots it lists are built; the others are built on
    /// demand when something resolves them from the open scope.
    #[doc(hidden)]
    pub async fn open_planned<T: Any + Send + Sync>(
        parent: Arc<ScopeContainer>,
        registry: Arc<ScopeRegistry>,
        layout: &Arc<SlotLayout>,
        plan: Option<&EntryPlan>,
        seeds: Vec<BoxedComponent>,
        generation_lease: Arc<T>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let scope = layout.scope();
        let generation_lease: GenerationLease = Some(generation_lease);

        validate_seeds(layout, &seeds)?;

        if !layout.constructs() && seeds.is_empty() {
            return Ok(Self::empty(scope, parent, registry, generation_lease));
        }

        let slot = match layout.captures() {
            true => ScopeResolverSlot::default(),
            false => ScopeResolverSlot::detached(),
        };
        let mut cx = ComponentConstructionContext::new_with_slot(
            scope,
            Some(Arc::clone(&parent)),
            Arc::clone(&registry),
            parent.inherited_resolvers(),
            slot,
        );

        cx.use_layout(Arc::clone(layout));
        cx.store_handles(HandleStorage::Fixed);

        for seed in seeds {
            cx.insert(seed)?;
        }

        for (index, spec) in layout.slots().iter().enumerate() {
            let component = &spec.component;
            let slot = SlotIdx::new(index);
            let Some(factory) = spec.factory else {
                continue;
            };

            if plan.is_some_and(|plan| !plan.builds(slot)) {
                continue;
            }

            Self::ensure_ancestor_sources(Some(&parent), &spec.dependencies).await?;

            debug!(component = %component.name, scope = scope.name(), "constructing component");

            cx.set_running(Running::Slot(slot));

            let built = factory.build(&mut cx).await;

            cx.set_running(Running::Unplanned);
            cx.insert_at(slot, built?)?;

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
        let externals = parent.inherited_resolvers();

        Arc::new_cyclic(|container| ScopeContainer {
            scope,
            store: ScopeStore::default(),
            parent: Some(parent),
            registry,
            resolver_base: externals,
            resolvers: std::sync::OnceLock::new(),
            source: ComponentSource {
                container: container.clone(),
            },
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
        let storage = match parent {
            Some(_) => HandleStorage::Fixed,
            None => HandleStorage::Swappable,
        };
        let mut cx = ComponentConstructionContext::new_with_slot(
            scope,
            parent,
            Arc::clone(&registry),
            externals,
            ScopeResolverSlot::default(),
        );

        cx.store_handles(storage);

        for seed in seeds {
            let type_id = seed.ty.type_id;

            cx.insert(seed)?;
            register_providers_for(&mut cx, &registry, type_id);
        }

        for descriptor in order {
            match descriptor.effective_factory()? {
                Some(factory) => {
                    debug!(component = %descriptor.name, scope = scope.name(), "constructing component");

                    let component = factory.build(&mut cx).await?;

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
    /// hold its own [`ComponentSource`] (a `Weak` self-reference).
    fn freeze(
        parts: ComponentConstructionParts,
        generation_lease: GenerationLease,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let slot = parts.slot;
        let container = Arc::new_cyclic(|weak| ScopeContainer {
            scope: parts.scope,
            store: parts.store,
            parent: parts.parent,
            registry: parts.registry,
            resolver_base: parts.resolvers,
            resolvers: std::sync::OnceLock::new(),
            source: ComponentSource {
                container: weak.clone(),
            },
            slot: slot.for_container(weak),
            generation_lease,
            drop_guard: std::sync::OnceLock::new(),
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

/// Checks that each seed fills its own factory-less slot of `layout`, so no seed is replaced
/// by a factory's output or by another seed.
fn validate_seeds(layout: &SlotLayout, seeds: &[BoxedComponent]) -> crate::Result<()> {
    let scope = layout.scope().name();

    for (index, seed) in seeds.iter().enumerate() {
        let type_id = seed.ty.type_id;
        let component = seed.ty.name;
        let slot = layout
            .slot_of(type_id)
            .ok_or(Error::UnplannedComponent { component, scope })?;

        if layout.spec(slot).factory.is_some() {
            return Err(Error::SeedForBuiltComponent { component, scope });
        }

        if seeds[..index]
            .iter()
            .any(|earlier| earlier.ty.type_id == type_id)
        {
            return Err(Error::DuplicateSeed { component, scope });
        }
    }

    Ok(())
}
