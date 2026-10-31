//! Opening scope instances through the prepared [`ScopeOpener`] protocols and users share.
//!
//! An opener checks once, when it is created, that its scope is declared, and resolves each
//! entry it opens for once. Opening an instance then pins a runtime generation, takes the
//! scope's prepared layout and builds the container, with no topology lookup or seed check
//! on the way.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{EntryKey, Scope, ScopeId, Singleton, TypeDescriptor};
use upwell_di::{BoxedComponent, ScopeContainer};

use super::{AppRuntime, RuntimeGeneration, RuntimeView};
use crate::entry::EntryId;
use crate::scope::ScopeParent;

/// Declares that scope `Self` may open under scope `P`.
///
/// Implement it for every parent the scope's topology lists, with [`Singleton`] standing for
/// the application root. [`ScopeBuilder::under`] requires it, so opening a scope under a
/// parent its topology does not list fails to compile.
pub trait OpensUnder<P: Scope>: Scope {}

/// The prepared opener of scope `S`, shared by protocols and user code.
///
/// Created once with [`AppRuntime::opener`], which checks that `S` is declared. Cloning it
/// bumps one reference count.
pub struct ScopeOpener<S: Scope> {
    runtime: Arc<AppRuntime>,
    scope: PhantomData<fn() -> S>,
}

/// An entry that runs in scope `S`, resolved once by [`ScopeOpener::entry`].
///
/// It holds only the entry's dense identity: opening reads the plan from the generation the
/// instance opens under, so a reload never leaves it stale.
pub struct ScopeEntry<S: Scope> {
    id: EntryId,
    scope: PhantomData<fn() -> S>,
}

/// An open instance of scope `S`, usable as a typed parent for [`ScopeBuilder::under`].
pub struct OpenScope<S: Scope> {
    container: Arc<ScopeContainer>,
    scope: PhantomData<fn() -> S>,
}

/// The inputs of one instance of scope `S` opened under a parent of scope `P`.
///
/// A stack value: it allocates nothing until a seed is added.
#[must_use = "a scope builder does nothing until it is opened"]
pub struct ScopeBuilder<'a, S: Scope, P: Scope = Singleton> {
    opener: &'a ScopeOpener<S>,
    parent: Option<&'a OpenScope<P>>,
    seeds: Vec<BoxedComponent>,
}

impl AppRuntime {
    /// The prepared opener of scope `S`.
    ///
    /// # Errors
    ///
    /// [`UndeclaredScopeOpen`](crate::Error::UndeclaredScopeOpen) when the active protocol
    /// does not declare `S`.
    pub fn opener<S: Scope>(&self) -> crate::Result<ScopeOpener<S>> {
        if self.scope_topology().boundary(&S::ID).is_none() {
            return Err(crate::Error::UndeclaredScopeOpen { scope: S::ID });
        }

        Ok(ScopeOpener {
            runtime: Arc::new(self.clone()),
            scope: PhantomData,
        })
    }

    /// The generation a child of `parent` opens under: the one `parent` runs on.
    fn pinned_view(&self, child: ScopeId, parent: &ScopeContainer) -> crate::Result<RuntimeView> {
        let Some(generation) = parent.generation_lease::<RuntimeGeneration>() else {
            let current = self.view();

            if !parent.belongs_to_registry(current.scopes()) {
                return Err(crate::Error::ForeignScopeParent {
                    child,
                    parent: parent.scope().id(),
                });
            }

            return Err(crate::Error::UnpinnedScopeParent {
                child,
                parent: parent.scope().id(),
            });
        };

        if !self.transitions.owns(&generation) {
            return Err(crate::Error::ForeignScopeParent {
                child,
                parent: parent.scope().id(),
            });
        }

        Ok(RuntimeView::from_generation(generation))
    }

    /// Builds an instance of `scope` under `parent` from the layout `view` prepared for it.
    async fn open_in(
        &self,
        view: RuntimeView,
        scope: ScopeId,
        parent: Arc<ScopeContainer>,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let layout = view
            .scope_plan()
            .layouts
            .get(&scope)
            .ok_or(crate::Error::UndeclaredScopeOpen { scope })?;

        ScopeContainer::open_planned(
            parent,
            Arc::clone(view.scopes()),
            layout,
            seeds,
            view.generation_state(),
        )
        .await
        .map_err(crate::Error::from)
    }
}

impl<S: Scope> ScopeOpener<S> {
    /// The entry with stable identity `key`, resolved once for every later opening.
    ///
    /// # Errors
    ///
    /// [`UnknownEntry`](crate::Error::UnknownEntry) when no entry has `key`, and
    /// [`EntryScopeMismatch`](crate::Error::EntryScopeMismatch) when it runs in another scope.
    pub fn entry(&self, key: EntryKey) -> crate::Result<ScopeEntry<S>> {
        let entries = &self.runtime.entries;
        let id = entries.id(&key).ok_or(crate::Error::UnknownEntry { key })?;
        let actual = entries
            .get(id)
            .expect("an entry table resolves the identities it assigned")
            .scope
            .id();

        if actual != S::ID {
            return Err(crate::Error::EntryScopeMismatch {
                key,
                expected: S::ID,
                actual,
            });
        }

        Ok(ScopeEntry {
            id,
            scope: PhantomData,
        })
    }

    /// Starts an instance under the application root; [`ScopeBuilder::under`] picks another
    /// parent.
    pub fn builder(&self) -> ScopeBuilder<'_, S> {
        ScopeBuilder {
            opener: self,
            parent: None,
            seeds: Vec::new(),
        }
    }
}

impl<S: Scope> Clone for ScopeOpener<S> {
    fn clone(&self) -> Self {
        Self {
            runtime: Arc::clone(&self.runtime),
            scope: PhantomData,
        }
    }
}

impl<S: Scope> ScopeEntry<S> {
    /// The entry's dense identity in the application's entry table.
    pub fn id(&self) -> EntryId {
        self.id
    }
}

impl<S: Scope> Clone for ScopeEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Scope> Copy for ScopeEntry<S> {}

impl<S: Scope> OpenScope<S> {
    /// The instance's container.
    pub fn container(&self) -> &Arc<ScopeContainer> {
        &self.container
    }

    /// The instance's container, giving up the scope type.
    pub fn into_container(self) -> Arc<ScopeContainer> {
        self.container
    }
}

impl<S: Scope> Clone for OpenScope<S> {
    fn clone(&self) -> Self {
        Self {
            container: Arc::clone(&self.container),
            scope: PhantomData,
        }
    }
}

impl<'a, S: Scope, P: Scope> ScopeBuilder<'a, S, P> {
    /// Opens the instance under `parent`, on the runtime generation `parent` runs on.
    pub fn under<Q: Scope>(self, parent: &'a OpenScope<Q>) -> ScopeBuilder<'a, S, Q>
    where
        S: OpensUnder<Q>,
    {
        ScopeBuilder {
            opener: self.opener,
            parent: Some(parent),
            seeds: self.seeds,
        }
    }

    /// Fills the scope's factory-less slot for `T` with `value`.
    pub fn seed<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.seeds.push(BoxedComponent {
            ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
            value: Box::new(value),
        });

        self
    }
}

impl<S: OpensUnder<P>, P: Scope> ScopeBuilder<'_, S, P> {
    /// Opens the instance to run `entry`.
    ///
    /// Every entry of a scope shares the scope's layout until entries carry their own plans
    /// (#348).
    ///
    /// # Errors
    ///
    /// Fails when the parent belongs to another runtime, a seed has no slot in the scope, or
    /// a component fails to build.
    pub async fn open(self, _entry: &ScopeEntry<S>) -> crate::Result<OpenScope<S>> {
        self.open_default().await
    }

    /// Opens the instance without an entry: it builds the scope's eager slots and resolves
    /// the rest on demand.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    pub async fn open_default(self) -> crate::Result<OpenScope<S>> {
        let runtime = &self.opener.runtime;
        let (view, parent) = match self.parent {
            Some(parent) => (
                runtime.pinned_view(S::ID, &parent.container)?,
                Arc::clone(&parent.container),
            ),
            None => {
                let view = runtime.view();
                let root = Arc::clone(view.root());

                (view, root)
            }
        };

        debug_assert!(
            opens_under::<S, P>(&view),
            "`OpensUnder` lists a parent the topology of `{}` does not",
            S::NAME
        );
        debug_assert!(
            validate_seeds(&view, S::ID, &self.seeds).is_ok(),
            "a seed of `{}` is duplicated or has no slot in it",
            S::NAME
        );

        let container = runtime.open_in(view, S::ID, parent, self.seeds).await?;

        Ok(OpenScope {
            container,
            scope: PhantomData,
        })
    }
}

/// Whether the topology `view` runs on lets `S` open under `P`.
fn opens_under<S: Scope, P: Scope>(view: &RuntimeView) -> bool {
    let parent = if P::ID == Singleton::ID {
        ScopeParent::Root
    } else {
        ScopeParent::of::<P>()
    };

    view.scope_plan()
        .topology
        .boundary(&S::ID)
        .is_some_and(|boundary| boundary.opens_under(parent))
}

/// Whether `seeds` are distinct and each fills a declared slot of `scope`.
pub(super) fn validate_seeds(
    view: &RuntimeView,
    scope: ScopeId,
    seeds: &[BoxedComponent],
) -> crate::Result<()> {
    for (index, seed) in seeds.iter().enumerate() {
        let type_id = seed.ty.type_id;
        let type_name = (seed.ty.type_name)();

        if seeds[..index]
            .iter()
            .any(|candidate| candidate.ty.type_id == type_id)
        {
            return Err(crate::Error::DuplicateSeedType { scope, type_name });
        }

        let Some(destination) = view.scope_plan().seed_destinations.get(&type_id) else {
            return Err(crate::Error::UnregisteredSeed { scope, type_name });
        };

        if destination.scope != scope {
            return Err(crate::Error::InvalidSeedDestination {
                type_name: destination.type_name,
                expected: destination.scope,
                actual: scope,
            });
        }
    }

    Ok(())
}
