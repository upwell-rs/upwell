use std::any::{Any, TypeId};
use std::sync::Arc;

use upwell_core::{ResolverCtx, ResolverSet, ScopeKey};

use super::planned::Running;
use super::{BoxedComponent, Component, Injectable, ProviderDescriptor, ScopeStore};
use crate::layout::SlotLayout;

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
    pub(super) scope: ScopeKey,
    /// The instances built so far. Shared when transients constructed mid-build resolve
    /// their own dependencies from this same in-progress store; reads and writes are brief
    /// and sequential, and no guard is held across an await. Empty until the first write,
    /// so a request-time extraction context, which never stores anything, allocates none.
    pub(super) store: BuildStore,
    pub(super) parent: Option<Arc<crate::container::ScopeContainer>>,
    pub(super) registry: Arc<crate::container::ScopeRegistry>,
    pub(super) slot: crate::container::ScopeResolverSlot,
    /// The pending slot created when a factory captures a scope opened with a detached slot,
    /// because its layout planned no capture.
    captured: std::sync::OnceLock<crate::container::ScopeResolverSlot>,
    /// The prepared layout this scope is built from, when it is opened from one.
    pub(super) layout: Option<Arc<SlotLayout>>,
    /// The factory being run, whose planned edges resolutions read first.
    pub(super) running: Running,
    /// External resolvers (e.g. the config store) threaded in by the runtime, so a
    /// factory parameter like `Cfg<T>` resolves through `cx.get::<ConfigStore>()`.
    resolvers: ResolverSet,
    /// The active instance a replacement factory replaces, set only while that factory
    /// runs during candidate construction.
    previous: Option<BoxedComponent>,
    /// The planned change a transition handler decides on, set only while that handler
    /// runs during candidate construction.
    change: Option<crate::ComponentChange>,
    /// How the handles built here are stored.
    storage: HandleStorage,
}

/// How a construction context stores the handles its factories build.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum HandleStorage {
    /// In a swappable cell a transition can republish, as the root stores its singletons.
    #[default]
    Swappable,
    /// As fixed values, for a scope below the root, whose instances no transition rebinds.
    Fixed,
}

const STORE_POISON: &str = "scope store lock poisoned";

/// A construction store shared with the transients built while it is in progress.
pub(crate) type SharedStore = Arc<std::sync::RwLock<ScopeStore>>;

/// Where a construction context keeps the instances it builds.
#[derive(Default)]
pub(super) enum BuildStore {
    /// Nothing stored yet, as in a request-time extraction context.
    #[default]
    Empty,
    /// Owned by the context, for a scope none of whose factories captures it, so no
    /// transient reads it while it is built.
    Owned(ScopeStore),
    /// Shared with the transients built while the scope is in progress.
    Shared(SharedStore),
}

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
        if kind == TypeId::of::<crate::container::ComponentSource>()
            && let Some(parent) = &self.parent
        {
            return Some(parent.source());
        }

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
            store: BuildStore::Empty,
            parent,
            registry,
            resolvers,
            slot,
            captured: std::sync::OnceLock::new(),
            layout: None,
            running: Running::Unplanned,
            previous: None,
            change: None,
            storage: HandleStorage::Swappable,
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
            store: BuildStore::Shared(store),
            parent,
            registry,
            resolvers,
            slot,
            captured: std::sync::OnceLock::new(),
            layout: None,
            running: Running::Unplanned,
            previous: None,
            change: None,
            storage: HandleStorage::Swappable,
        }
    }

    /// Stores the handles built from now on as `storage` says.
    pub(crate) fn store_handles(&mut self, storage: HandleStorage) {
        self.storage = storage;
    }

    /// Wraps a handle a factory built into the value this context's scope stores.
    #[doc(hidden)]
    pub fn stored<H: Injectable>(&self, handle: H) -> H::Stored {
        match self.storage {
            HandleStorage::Swappable => handle.into_stored(),
            HandleStorage::Fixed => handle.into_fixed(),
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
        match self.slot.is_detached() {
            true => self
                .captured
                .get_or_init(crate::container::ScopeResolverSlot::default)
                .clone(),
            false => self.slot.clone(),
        }
    }

    /// The shared handle to this context's store, for transient construction. A
    /// context that has stored nothing hands out a fresh empty store: a transient
    /// only reads it while it is built, so it observes the same empty state. An owned
    /// store belongs to a scope whose layout plans no transient, so a transient built
    /// against it dynamically sees an empty store as well.
    pub(crate) fn store_handle(&self) -> SharedStore {
        match &self.store {
            BuildStore::Shared(store) => Arc::clone(store),
            BuildStore::Empty | BuildStore::Owned(_) => SharedStore::default(),
        }
    }

    /// Reads this context's store, or `None` when nothing has been stored yet.
    pub(super) fn read_store<R>(&self, read: impl FnOnce(&ScopeStore) -> Option<R>) -> Option<R> {
        match &self.store {
            BuildStore::Empty => None,
            BuildStore::Owned(store) => read(store),
            BuildStore::Shared(store) => read(&store.read().expect(STORE_POISON)),
        }
    }

    /// Writes this context's store. The first write to an empty context shares it, so
    /// the transients built against a scope without a layout see what it holds.
    fn write_store<R>(&mut self, write: impl FnOnce(&mut ScopeStore) -> R) -> R {
        if matches!(self.store, BuildStore::Empty) {
            self.store = BuildStore::Shared(SharedStore::default());
        }

        match &mut self.store {
            BuildStore::Owned(store) => write(store),
            BuildStore::Shared(store) => write(&mut store.write().expect(STORE_POISON)),
            BuildStore::Empty => unreachable!("an empty store was just shared"),
        }
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

    pub(crate) fn insert(&mut self, component: BoxedComponent) -> crate::Result<()> {
        self.write_store(|store| store.insert(component))
    }

    /// Stores what a planned slot's own factory built, at that slot.
    pub(crate) fn insert_at(
        &mut self,
        slot: crate::layout::SlotIdx,
        component: BoxedComponent,
    ) -> crate::Result<()> {
        self.write_store(|store| store.insert_at(slot, component))
    }

    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        self.write_store(|store| store.register_provider(provider, ordinal));
    }

    pub(crate) fn into_parts(self) -> ComponentConstructionParts {
        // The scope is freezing: every transient context sharing this store was
        // dropped when its construction returned, so unwrapping cannot race.
        let store = match self.store {
            BuildStore::Empty => ScopeStore::default(),
            BuildStore::Owned(store) => store,
            BuildStore::Shared(store) => match Arc::try_unwrap(store) {
                Ok(lock) => lock.into_inner().expect(STORE_POISON),
                Err(_) => panic!("construction store has outstanding readers at scope freeze"),
            },
        };

        ComponentConstructionParts {
            scope: self.scope,
            store,
            parent: self.parent,
            registry: self.registry,
            resolvers: self.resolvers,
            slot: self.captured.into_inner().unwrap_or(self.slot),
        }
    }

    /// Whether a component of `type_id` has already been constructed or seeded in
    /// this scope.
    pub(crate) fn contains(&self, type_id: TypeId) -> bool {
        self.read_store(|store| store.contains(type_id).then_some(()))
            .is_some()
    }
}
