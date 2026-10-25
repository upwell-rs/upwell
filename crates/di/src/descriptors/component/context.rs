use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolutionMode, ResolverCtx, ResolverSet, ScopeKey};

use super::{BoxedComponent, Component, Injectable, ProviderDescriptor, ScopeStore};

/// Context passed to a component factory during construction.
///
/// Holds the components built so far in this scope plus a link to the parent scope
/// chain, so a factory can resolve dependencies from its own scope or any
/// longer-lived one. Factories call `resolve`/`resolve_all`/`resolve_keyed` to
/// obtain their component dependencies; *external* sources (config) are reached
/// through the [`ResolverCtx`] impl (`cx.get::<ConfigStore>()`).
///
/// Resolution is `async` even though eagerly-built scopes resolve immediately
/// (the future is ready): a `Transient` dependency is constructed on demand here,
/// and keeping the interface async is what lets connection/request scopes move to
/// lazy construction later without changing generated factory code.
pub struct ComponentConstructionContext {
    scope: ScopeKey,
    /// Shared because transients constructed mid-build resolve their own
    /// dependencies from this same in-progress store. Reads/writes are brief and
    /// sequential; no guard is held across an await. `None` until the first write,
    /// so a request-time extraction context, which never stores anything, does not
    /// allocate one.
    store: Option<SharedStore>,
    parent: Option<Arc<crate::container::ScopeContainer>>,
    registry: Arc<crate::container::ScopeRegistry>,
    slot: crate::container::ScopeResolverSlot,
    /// External resolvers (e.g. the config store) threaded in by the runtime, so a
    /// factory parameter like `Cfg<T>` resolves through `cx.get::<ConfigStore>()`.
    resolvers: ResolverSet,
    /// The active instance a replacement factory replaces, set only while that factory
    /// runs during candidate construction.
    previous: Option<BoxedComponent>,
    /// The planned change a transition handler decides on, set only while that handler
    /// runs during candidate construction.
    change: Option<crate::ComponentChange>,
}

const STORE_POISON: &str = "scope store lock poisoned";

/// A construction store shared with the transients built while it is in progress.
pub(crate) type SharedStore = Arc<std::sync::RwLock<ScopeStore>>;

pub(crate) struct ComponentConstructionParts {
    pub(crate) scope: ScopeKey,
    pub(crate) store: ScopeStore,
    pub(crate) parent: Option<Arc<crate::container::ScopeContainer>>,
    pub(crate) registry: Arc<crate::container::ScopeRegistry>,
    pub(crate) resolvers: ResolverSet,
    pub(crate) slot: crate::container::ScopeResolverSlot,
}

impl ResolverCtx for ComponentConstructionContext {
    fn resolver(&self, kind: TypeId) -> Option<&dyn Any> {
        self.resolvers.resolver(kind)
    }
}

impl ComponentConstructionContext {
    pub(crate) fn new_with_slot(
        scope: ScopeKey,
        parent: Option<Arc<crate::container::ScopeContainer>>,
        registry: Arc<crate::container::ScopeRegistry>,
        resolvers: ResolverSet,
        slot: crate::container::ScopeResolverSlot,
    ) -> Self {
        Self {
            scope,
            store: None,
            parent,
            registry,
            resolvers,
            slot,
            previous: None,
            change: None,
        }
    }

    /// Creates a context over an existing store, used when a transient is
    /// constructed while another scope is still building: its dependencies must
    /// resolve from the components already built in that scope, not just the
    /// frozen parent chain.
    pub(crate) fn with_store(
        scope: ScopeKey,
        parent: Option<Arc<crate::container::ScopeContainer>>,
        registry: Arc<crate::container::ScopeRegistry>,
        resolvers: ResolverSet,
        slot: crate::container::ScopeResolverSlot,
        store: SharedStore,
    ) -> Self {
        Self {
            scope,
            store: Some(store),
            parent,
            registry,
            resolvers,
            slot,
            previous: None,
            change: None,
        }
    }

    /// Sets the previous instance read by the next replacement factory.
    pub(crate) fn set_previous(&mut self, previous: Option<BoxedComponent>) {
        self.previous = previous;
    }

    /// Whether a previous instance is set for the running factory.
    pub(crate) fn has_previous(&self) -> bool {
        self.previous.is_some()
    }

    /// Sets the planned change read by the next transition handler.
    pub(crate) fn set_change(&mut self, change: Option<crate::ComponentChange>) {
        self.change = change;
    }

    /// The planned change the running transition handler decides on.
    pub(crate) fn change(&self) -> Option<&crate::ComponentChange> {
        self.change.as_ref()
    }

    /// The previous instance's handle for the component `C` a replacement factory is
    /// building.
    pub(crate) fn previous_handle<C: Component>(&self) -> crate::Result<C::Handle> {
        let unavailable = crate::Error::PreviousUnavailable { component: C::ID };
        let Some(previous) = &self.previous else {
            return Err(unavailable);
        };

        previous
            .value
            .downcast_ref::<<C::Handle as Injectable>::Stored>()
            .map(<C::Handle as Injectable>::from_stored)
            .ok_or(unavailable)
    }

    pub(crate) fn resolver_slot(&self) -> crate::container::ScopeResolverSlot {
        self.slot.clone()
    }

    /// The shared handle to this context's store, for transient construction. A
    /// context that has stored nothing hands out a fresh empty store: a transient
    /// only reads it while it is built, so it observes the same empty state.
    pub(crate) fn store_handle(&self) -> SharedStore {
        self.store.clone().unwrap_or_default()
    }

    /// Reads this context's store, or `None` when nothing has been stored yet.
    fn read_store<R>(&self, read: impl FnOnce(&ScopeStore) -> Option<R>) -> Option<R> {
        read(&self.store.as_ref()?.read().expect(STORE_POISON))
    }

    fn write_store(&mut self) -> std::sync::RwLockWriteGuard<'_, ScopeStore> {
        self.store
            .get_or_insert_with(SharedStore::default)
            .write()
            .expect(STORE_POISON)
    }

    /// Captures this construction scope for forced reconstruction of `H`.
    #[doc(hidden)]
    pub fn fresh<H: crate::FreshFromContainer>(
        &self,
        qualifier: Option<&'static str>,
    ) -> crate::Fresh<H> {
        crate::Fresh::capture(self.resolver_slot(), qualifier)
    }

    /// Registers a weak deferred target for hydration after this scope finishes construction.
    #[doc(hidden)]
    pub fn deferred<T>(&self, qualifier: Option<&'static str>) -> crate::Result<crate::Deferred<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        crate::Deferred::capture(self.resolver_slot(), qualifier)
    }

    /// Captures this construction scope for qualified lazy trait resolution.
    #[doc(hidden)]
    pub fn lazy_qualified<T>(&self, qualifier: &'static str) -> crate::Lazy<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        crate::Lazy::capture_qualified(self.resolver_slot(), qualifier)
    }

    /// Resolves a single dependency by its injectable handle `H`, keyed under
    /// `H::Target`: this scope first, then each longer-lived parent scope, then —
    /// if `H::Target` is a `Transient` component — a freshly constructed instance.
    pub async fn resolve<H: Injectable>(&self) -> crate::Result<Option<H>> {
        let target = TypeId::of::<H::Target>();

        if let Some(provider) = self.registry.selected_runtime_provider(
            target,
            None,
            ResolutionMode::Eager,
            self.scope,
            &|scope| self.can_access(scope),
        ) {
            return self.resolve_selected::<H>(provider).await;
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

    /// Qualifier-selected single provider, this scope then parents.
    pub async fn resolve_qualified<H: Injectable>(
        &self,
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

        self.resolve_selected::<H>(provider).await
    }

    /// Every provider of the trait `H::Target` across this scope and its parents.
    pub async fn resolve_all<H: Injectable>(&self) -> crate::Result<Vec<H>> {
        let target = TypeId::of::<H::Target>();
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

    /// Every provider of the trait `H::Target` keyed by qualifier, across this scope
    /// and its parents (a closer scope wins a qualifier collision).
    pub async fn resolve_keyed<H: Injectable>(&self) -> crate::Result<HashMap<String, H>> {
        let target = TypeId::of::<H::Target>();
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

    pub(crate) fn insert(&mut self, component: BoxedComponent) {
        self.write_store().insert(component);
    }

    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        self.write_store().register_provider(provider, ordinal);
    }

    pub(crate) fn into_parts(self) -> ComponentConstructionParts {
        // The scope is freezing: every transient context sharing this store was
        // dropped when its construction returned, so unwrapping cannot race.
        let store = match self.store.map(Arc::try_unwrap) {
            None => ScopeStore::default(),
            Some(Ok(lock)) => lock.into_inner().expect(STORE_POISON),
            Some(Err(_)) => panic!("construction store has outstanding readers at scope freeze"),
        };

        ComponentConstructionParts {
            scope: self.scope,
            store,
            parent: self.parent,
            registry: self.registry,
            resolvers: self.resolvers,
            slot: self.slot,
        }
    }

    /// Whether a component of `type_id` has already been constructed or seeded in
    /// this scope.
    pub(crate) fn contains(&self, type_id: TypeId) -> bool {
        self.read_store(|store| store.contains(type_id).then_some(()))
            .is_some()
    }
}
