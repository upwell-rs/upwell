use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
};

use tracing::{debug, error, info, instrument, trace};
use upwell_core::{
    ResolutionMode, Resolver, ResolverCtx, ResolverSet, Scope, ScopeId, Singleton, Transient,
};

use crate::descriptors::BoxedComponent;
use crate::registry::selection::ProviderSelectionModel;
use crate::{
    descriptors::{
        Component, ComponentDescriptor, Injectable, ProviderDescriptor,
        component::{ComponentConstructionContext, ScopeStore},
    },
    error::Error,
};

mod candidate;
mod snapshot;

/// Shared, immutable data a [`ScopeContainer`] needs to resolve beyond its own
/// store: the `Transient` components it may construct on demand and the trait
/// providers used to alias instances. Held behind an `Arc` and shared by every
/// scope in an application (root, per-connection, per-request).
pub struct ScopeRegistry {
    /// Transient components keyed by their concrete `TypeId`, for on-demand
    /// construction at resolution time. Transients are never cached.
    transient: HashMap<TypeId, ComponentDescriptor>,
    factory_backed: HashMap<TypeId, ComponentDescriptor>,
    selection: Arc<ProviderSelectionModel>,
}

impl ScopeRegistry {
    pub fn new(
        transient: HashMap<TypeId, ComponentDescriptor>,
        components: HashMap<TypeId, ComponentDescriptor>,
        providers: Vec<ProviderDescriptor>,
        provider_order: HashMap<TypeId, HashMap<TypeId, usize>>,
    ) -> crate::Result<Self> {
        let selection_components = components
            .values()
            .copied()
            .chain(transient.values().copied())
            .collect::<Vec<_>>();
        let selection = Arc::new(ProviderSelectionModel::new(
            &selection_components,
            providers,
            provider_order,
        )?);

        Self::from_selection_model(transient, components, selection)
    }

    /// Creates a runtime registry from the provider model retained by validation
    /// and construction planning.
    pub fn from_selection_model(
        transient: HashMap<TypeId, ComponentDescriptor>,
        components: HashMap<TypeId, ComponentDescriptor>,
        selection: Arc<ProviderSelectionModel>,
    ) -> crate::Result<Self> {
        let factory_backed = components
            .values()
            .filter(|component| component.effective_factory().ok().flatten().is_some())
            .map(|component| (component.ty.type_id, *component))
            .collect();

        for component in transient.values().chain(components.values()) {
            if selection.component(component.ty.type_id).is_none() {
                return Err(Error::MissingComponent(component.name));
            }
        }

        Ok(Self {
            transient,
            factory_backed,
            selection,
        })
    }

    pub(crate) fn providers_for(&self, concrete: TypeId) -> &[ProviderDescriptor] {
        self.selection.providers_for_concrete(concrete)
    }

    pub(crate) fn selected_runtime_provider(
        &self,
        trait_id: TypeId,
        qualifier: Option<&str>,
        resolution: ResolutionMode,
        scope: &dyn Scope,
        can_access: &impl Fn(&'static dyn Scope) -> bool,
    ) -> Option<ProviderDescriptor> {
        self.selection
            .select_runtime_one(trait_id, qualifier, resolution, scope, &|_, dependency| {
                can_access(dependency)
            })
            .map(|selection| selection.provider)
    }

    pub(crate) fn selected_runtime_collection(
        &self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        scope: &dyn Scope,
        can_access: &impl Fn(&'static dyn Scope) -> bool,
    ) -> Vec<ProviderDescriptor> {
        self.selection
            .select_runtime_collection(trait_id, resolution, scope, &|_, dependency| {
                can_access(dependency)
            })
            .into_iter()
            .map(|selection| selection.provider)
            .collect()
    }

    pub(crate) fn selected_runtime_keyed(
        &self,
        trait_id: TypeId,
        resolution: ResolutionMode,
        scope: &dyn Scope,
        can_access: &impl Fn(&'static dyn Scope) -> bool,
    ) -> Vec<ProviderDescriptor> {
        self.selection
            .select_runtime_keyed(trait_id, resolution, scope, &|_, dependency| {
                can_access(dependency)
            })
            .into_iter()
            .map(|selection| selection.provider)
            .collect()
    }

    pub(crate) fn provider_ordinal(&self, provider: &ProviderDescriptor) -> usize {
        self.selection.ordinal(provider)
    }

    pub(crate) fn transient(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.transient.get(&target).copied()
    }

    pub(crate) fn factory_backed(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.factory_backed.get(&target).copied()
    }

    pub(crate) fn component(&self, target: TypeId) -> Option<ComponentDescriptor> {
        self.selection.component(target)
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = ComponentDescriptor> + '_ {
        self.selection.components()
    }

    pub(crate) fn component_by_id(&self, id: &str) -> Option<ComponentDescriptor> {
        self.components().find(|component| component.id == id)
    }
}

/// Cloneable attachment point for a scope under construction or already built.
#[derive(Clone)]
pub(crate) struct ScopeResolverSlot {
    state: ScopeResolverSlotState,
}

/// Storage used by a pending or attached scope resolver slot.
#[derive(Clone)]
enum ScopeResolverSlotState {
    Pending(Arc<std::sync::Mutex<ScopeResolverState>>),
    Attached(Weak<ScopeContainer>),
}

/// Mutable hydration state shared while a scope is under construction.
#[derive(Default)]
struct ScopeResolverState {
    container: Weak<ScopeContainer>,
    deferred: Vec<Arc<dyn crate::primitives::DeferredHydrator>>,
}

impl Default for ScopeResolverSlot {
    fn default() -> Self {
        Self {
            state: ScopeResolverSlotState::Pending(Arc::new(std::sync::Mutex::new(
                ScopeResolverState::default(),
            ))),
        }
    }
}

impl ScopeResolverSlot {
    fn attached(container: Weak<ScopeContainer>) -> Self {
        Self {
            state: ScopeResolverSlotState::Attached(container),
        }
    }

    pub(crate) fn attach(&self, container: &Arc<ScopeContainer>) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            return Ok(());
        };
        let deferred = {
            let mut state = state.lock().expect("scope resolver slot poisoned");

            state.container = Arc::downgrade(container);

            std::mem::take(&mut state.deferred)
        };

        for deferred in deferred {
            deferred.hydrate(container)?;
        }

        Ok(())
    }

    pub(crate) fn resolve(&self) -> crate::Result<Arc<ScopeContainer>> {
        match &self.state {
            ScopeResolverSlotState::Pending(state) => state
                .lock()
                .expect("scope resolver slot poisoned")
                .container
                .upgrade()
                .ok_or(Error::ScopeUnavailable),
            ScopeResolverSlotState::Attached(container) => {
                container.upgrade().ok_or(Error::ScopeUnavailable)
            }
        }
    }

    pub(crate) fn register_deferred(
        &self,
        deferred: Arc<dyn crate::primitives::DeferredHydrator>,
    ) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            let container = self.resolve()?;

            return deferred.hydrate(&container);
        };
        let container = {
            let mut state = state.lock().expect("scope resolver slot poisoned");

            if let Some(container) = state.container.upgrade() {
                Some(container)
            } else {
                state.deferred.push(Arc::clone(&deferred));

                None
            }
        };

        if let Some(container) = container {
            deferred.hydrate(&container)?;
        }

        Ok(())
    }
}

/// A fresh empty shared store for transient construction against an already
/// frozen scope: resolution then flows entirely through the parent chain.
fn empty_store() -> Arc<std::sync::RwLock<ScopeStore>> {
    Arc::new(std::sync::RwLock::new(ScopeStore::default()))
}

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
    store: Arc<std::sync::RwLock<ScopeStore>>,
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
    store: Arc<std::sync::RwLock<ScopeStore>>,
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
    store: Arc<std::sync::RwLock<ScopeStore>>,
    provider: ProviderDescriptor,
) -> crate::Result<Option<H>> {
    let Some(descriptor) = registry.transient(provider.concrete_ty.type_id) else {
        return Ok(None);
    };
    let concrete = construct_transient_boxed(registry, parent, slot, store, descriptor).await?;
    let erased = (provider.erase)(&concrete);

    Ok(crate::descriptors::component::from_boxed::<H>(&erased))
}

/// One scope's constructed instances, layered over an optional parent scope.
///
/// The root container is the singleton scope (`parent: None`); a per-connection
/// scope parents the root, and a per-request scope parents the connection.
/// Resolution walks this scope first, then each longer-lived parent, so a request
/// handler sees request-, connection-, and singleton-scoped instances uniformly.
///
/// The container is also a [`ResolverCtx`]: it exposes a [`ComponentSource`] (for
/// resolving components by type — used by hooks and config-targeted resolution) plus
/// any *external* resolvers (the config store) threaded in at build.
pub struct ScopeContainer {
    scope: &'static dyn Scope,
    store: ScopeStore,
    parent: Option<Arc<ScopeContainer>>,
    registry: Arc<ScopeRegistry>,
    resolver_base: ResolverSet,
    resolvers: std::sync::OnceLock<ResolverSet>,
    slot: ScopeResolverSlot,
    generation_lease: Option<Arc<dyn Any + Send + Sync>>,
}

impl ResolverCtx for ScopeContainer {
    fn resolver(&self, kind: TypeId) -> Option<&dyn Any> {
        self.resolvers().resolver(kind)
    }
}

/// A [`Resolver`] over a [`ScopeContainer`], resolving components by type.
///
/// Held in the container's own resolver set under a [`Weak`] back-reference (so it
/// adds no reference cycle), it is how a hook reaches its `&self` receiver through
/// the erased `&dyn ResolverCtx` it is handed: `ctx.get::<ComponentSource>()?.component::<Self>()`.
pub struct ComponentSource {
    container: Weak<ScopeContainer>,
}

impl Resolver for ComponentSource {}

impl ComponentSource {
    /// The component of type `C` as its handle (`Arc<C>` or the by-value handle),
    /// resolved through the backing scope and its parents.
    pub fn component<C: Component>(&self) -> Option<C::Handle> {
        self.container.upgrade()?.get::<C>()
    }

    /// The handle `H` resolved through the backing scope and its parents.
    pub fn resolve<H: Injectable>(&self) -> Option<H> {
        self.container.upgrade()?.resolve_built::<H>()
    }
}

impl ScopeContainer {
    /// The scope this container holds.
    pub fn scope(&self) -> &'static dyn Scope {
        self.scope
    }

    /// The external resolvers threaded into this scope (config store, …), shared with
    /// child scopes.
    pub fn resolvers(&self) -> &ResolverSet {
        self.resolvers.get_or_init(|| {
            let container = self
                .slot
                .resolve()
                .expect("built scope resolver slot remains attached");
            let mut resolvers = self.resolver_base.clone();

            resolvers.insert(Arc::new(ComponentSource {
                container: Arc::downgrade(&container),
            }));

            resolvers
        })
    }

    pub(crate) fn registry(&self) -> Arc<ScopeRegistry> {
        Arc::clone(&self.registry)
    }

    /// Returns whether this container was built from `registry`.
    ///
    /// Runtime layers use this before opening a child to prevent containers from
    /// different prepared applications from being joined into one scope chain.
    pub fn belongs_to_registry(&self, registry: &Arc<ScopeRegistry>) -> bool {
        Arc::ptr_eq(&self.registry, registry)
    }

    /// Returns the runtime-generation lease retained by this scope when its type matches `T`.
    #[doc(hidden)]
    pub fn generation_lease<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        Arc::clone(self.generation_lease.as_ref()?).downcast().ok()
    }

    pub(crate) fn can_access(&self, scope: &'static dyn Scope) -> bool {
        if scope.is_transient() || self.scope.id() == scope.id() {
            return true;
        }

        self.parent
            .as_ref()
            .is_some_and(|parent| parent.can_access(scope))
    }

    /// Returns the container whose identity matches `scope` from this scope's
    /// ancestry. Fresh construction uses this boundary as its resolution root so
    /// the rebuilt target cannot observe components from its shorter-lived owner.
    fn container_for_scope(
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
    async fn build(
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
            }
        });
        slot.attach(&container)?;

        Ok(container)
    }

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
                    empty_store(),
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
            empty_store(),
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
                empty_store(),
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
            &Transient,
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

/// Backwards-compatible alias: the root singleton store is a [`ScopeContainer`]
/// with no parent.
pub type ComponentContainer = ScopeContainer;

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

/// Computes a construction order using the same immutable provider selections as
/// validation and runtime resolution.
///
/// Every `TypeId` in `prebuilt` is already available from seeded instances or a
/// parent scope. `can_reach` describes scope ancestry. Config, dynamic, optional,
/// and non-eager edges impose no construction constraint.
pub fn topological_sort<'a>(
    components: &'a [ComponentDescriptor],
    prebuilt: &HashSet<TypeId>,
    selection: &ProviderSelectionModel,
    can_reach: impl Fn(ScopeId, ScopeId) -> bool,
) -> crate::Result<Vec<&'a ComponentDescriptor>> {
    trace!(total = components.len(), "starting topological sort");

    let sortable: HashSet<TypeId> = components.iter().map(|c| c.ty.type_id).collect();
    let can_access = |consumer: &dyn Scope, dependency: &'static dyn Scope| {
        if dependency.is_transient() {
            return true;
        }

        can_reach(consumer.id(), dependency.id())
    };
    let mut expansion = WaitExpansion {
        selection,
        sortable: &sortable,
        components,
        can_access: &can_access,
        provider_memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    let waits = components
        .iter()
        .map(|descriptor| {
            (
                descriptor.ty.type_id,
                expansion.component_waits(*descriptor),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut result: Vec<&'a ComponentDescriptor> = Vec::new();
    let mut remaining: Vec<&'a ComponentDescriptor> = components.iter().collect();

    while !remaining.is_empty() {
        let before_len = remaining.len();

        remaining.retain(|descriptor| {
            let is_built = |type_id: TypeId| {
                prebuilt.contains(&type_id) || result.iter().any(|r| r.ty.type_id == type_id)
            };

            let resolved = waits[&descriptor.ty.type_id]
                .iter()
                .all(|dependency| is_built(*dependency));

            if resolved {
                result.push(descriptor);
                crate::observability::build_position(descriptor, result.len() - 1);
                false
            } else {
                true
            }
        });

        if remaining.len() == before_len {
            let stuck = remaining
                .iter()
                .map(|d| d.name)
                .collect::<Vec<_>>()
                .join(", ");
            for diagnostics in construction_cycle_diagnostics(components, &waits, &remaining) {
                crate::observability::construction_cycle(
                    diagnostics.cycle_id,
                    &diagnostics.members,
                    &diagnostics.edges,
                    &diagnostics.blocked,
                );
            }

            error!(components = %stuck, "dependency cycle detected in component graph");

            return Err(Error::DependencyCycle(stuck));
        }
    }

    trace!(count = result.len(), "topological sort complete");

    Ok(result)
}

/// Transitive wait-set expansion for one scope's construction sort.
struct WaitExpansion<'a> {
    selection: &'a ProviderSelectionModel,
    sortable: &'a HashSet<TypeId>,
    components: &'a [ComponentDescriptor],
    can_access: &'a dyn Fn(&dyn Scope, &'static dyn Scope) -> bool,
    provider_memo: HashMap<TypeId, HashSet<TypeId>>,
    visiting: HashSet<TypeId>,
}

impl WaitExpansion<'_> {
    fn component_waits(&mut self, descriptor: ComponentDescriptor) -> HashSet<TypeId> {
        let mut waits = HashSet::new();

        for dependency in descriptor.dependencies() {
            let reason = construction_edge_reason(&dependency);
            let accepted = reason == "required-eager";

            crate::observability::construction_edge(&descriptor, &dependency, accepted, reason);

            if accepted {
                self.expand_dependency(descriptor.scope, &dependency, &mut waits);
            }
        }

        let mut ordered = waits.iter().copied().collect::<Vec<_>>();
        ordered.sort_by_key(|type_id| component_id(self.components, *type_id));

        for dependency in ordered {
            if let Some(target) = self
                .components
                .iter()
                .find(|component| component.ty.type_id == dependency)
            {
                crate::observability::construction_wait(&descriptor, target);
            }
        }

        waits
    }

    /// The sortable TypeIds a single provider concrete gates: itself when built
    /// in this scope, or the eager dependencies of its transient recipe.
    fn provider_waits(&mut self, concrete: TypeId) -> HashSet<TypeId> {
        if let Some(done) = self.provider_memo.get(&concrete) {
            return done.clone();
        }

        let mut waits = HashSet::new();

        if self.sortable.contains(&concrete) {
            waits.insert(concrete);
        } else if let Some(descriptor) = self
            .selection
            .component(concrete)
            .filter(|descriptor| descriptor.scope.is_transient())
        {
            self.expand_transient(descriptor, &mut waits);
        }

        self.provider_memo.insert(concrete, waits.clone());

        waits
    }

    fn expand_transient(&mut self, descriptor: ComponentDescriptor, waits: &mut HashSet<TypeId>) {
        if !self.visiting.insert(descriptor.ty.type_id) {
            return;
        }

        for dependency in descriptor.dependencies().into_iter().filter(|dependency| {
            !dependency.optional
                && !dependency.dynamic
                && !dependency.config
                && dependency.resolution == ResolutionMode::Eager
        }) {
            self.expand_dependency(descriptor.scope, &dependency, waits);
        }

        self.visiting.remove(&descriptor.ty.type_id);
    }

    fn expand_dependency(
        &mut self,
        consumer: &'static dyn Scope,
        dependency: &upwell_core::DependencyDescriptor,
        waits: &mut HashSet<TypeId>,
    ) {
        if let Some(component) = self.selection.component(dependency.ty.type_id) {
            if (self.can_access)(consumer, component.scope) {
                waits.extend(self.provider_waits(component.ty.type_id));
            } else {
                waits.insert(component.ty.type_id);
            }

            return;
        }

        let providers = self.selection.construction_providers(
            dependency,
            consumer,
            self.can_access,
            &|concrete| self.sortable.contains(&concrete),
        );

        for provider in providers {
            waits.extend(self.provider_waits(provider.concrete_ty.type_id));
        }
    }
}

fn construction_edge_reason(dependency: &upwell_core::DependencyDescriptor) -> &'static str {
    if dependency.optional {
        return "optional";
    }

    if dependency.dynamic {
        return "dynamic";
    }

    if dependency.config {
        return "config-external";
    }

    match dependency.resolution {
        ResolutionMode::Eager => "required-eager",
        ResolutionMode::Lazy => "lazy",
        ResolutionMode::Deferred => "deferred-cycle-break",
        ResolutionMode::Fresh => "fresh",
    }
}

struct CycleDiagnostics<'a> {
    cycle_id: &'a str,
    members: Vec<&'a str>,
    edges: Vec<(&'a str, &'a str)>,
    blocked: Vec<&'a str>,
}

fn construction_cycle_diagnostics<'a>(
    components: &'a [ComponentDescriptor],
    waits: &HashMap<TypeId, HashSet<TypeId>>,
    remaining: &[&ComponentDescriptor],
) -> Vec<CycleDiagnostics<'a>> {
    let remaining_ids = remaining
        .iter()
        .map(|component| component.ty.type_id)
        .collect::<Vec<_>>();
    let keys = components
        .iter()
        .map(|component| (component.ty.type_id, component.id.to_string()))
        .collect::<HashMap<_, _>>();
    let cycles = crate::registry::order::cycle::components(&remaining_ids, waits, &keys);
    let all_cyclic = cycles.iter().flatten().copied().collect::<HashSet<_>>();

    cycles
        .into_iter()
        .map(|cycle| {
            let cycle_set = cycle.iter().copied().collect::<HashSet<_>>();
            let mut members = cycle
                .iter()
                .map(|type_id| component_id(components, *type_id))
                .collect::<Vec<_>>();
            let mut edges = cycle
                .iter()
                .flat_map(|from| {
                    waits[from]
                        .iter()
                        .filter(|to| cycle_set.contains(to))
                        .map(|to| {
                            (
                                component_id(components, *from),
                                component_id(components, *to),
                            )
                        })
                })
                .collect::<Vec<_>>();
            let mut blocked = remaining_ids
                .iter()
                .filter(|candidate| !all_cyclic.contains(candidate))
                .filter(|candidate| transitively_waits_on(**candidate, &cycle_set, waits))
                .map(|type_id| component_id(components, *type_id))
                .collect::<Vec<_>>();

            members.sort_unstable();
            edges.sort_unstable();
            blocked.sort_unstable();

            CycleDiagnostics {
                cycle_id: members.first().copied().unwrap_or(""),
                members,
                edges,
                blocked,
            }
        })
        .collect()
}

fn transitively_waits_on(
    component: TypeId,
    targets: &HashSet<TypeId>,
    waits: &HashMap<TypeId, HashSet<TypeId>>,
) -> bool {
    let mut pending = vec![component];
    let mut visited = HashSet::new();

    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }

        for dependency in waits.get(&current).into_iter().flatten() {
            if targets.contains(dependency) {
                return true;
            }

            pending.push(*dependency);
        }
    }

    false
}

fn component_id(components: &[ComponentDescriptor], type_id: TypeId) -> &str {
    components
        .iter()
        .find(|component| component.ty.type_id == type_id)
        .map_or("<external>", |component| component.id)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod observability_tests;
