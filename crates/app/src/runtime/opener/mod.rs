//! Opening scope instances through the prepared [`ScopeOpener`] protocols and users share.
//!
//! An opener checks once, when it is created, that its scope is declared, and resolves each
//! entry it opens for once. Opening an instance then pins a runtime generation, takes the
//! scope's prepared layout and builds the container. The parent's place in the topology is
//! checked only in debug builds; each seed is checked against the slot it fills.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{EntryKey, Scope, ScopeId, Singleton, TypeDescriptor};
use upwell_di::{BoxedComponent, EntryPlan, ScopeContainer};

use super::{AppRuntime, RuntimeGeneration, RuntimeView};
use crate::entry::EntryId;
use crate::scope::ScopeParent;

mod entry_scope;

pub use entry_scope::EntryScope;

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

/// The slots opening an instance builds at once; the rest are built on demand.
#[derive(Clone, Copy, Debug)]
enum Builds {
    /// What the entry's plan lists, or every slot when the generation has no plan for it.
    Entry(EntryId),
    /// Every slot of the scope.
    Everything,
    /// Only the seeds.
    Nothing,
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
    /// Opens the instance to run `entry`: it builds what the entry's plan lists, the entry's
    /// targets and what they depend on in the scope, and builds any other slot on demand when
    /// something resolves it.
    ///
    /// # Errors
    ///
    /// Fails when the parent belongs to another runtime, a seed has no slot in the scope or
    /// shares one with another seed, or a component fails to build.
    pub async fn open(self, entry: &ScopeEntry<S>) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Entry(entry.id)).await
    }

    /// Opens the instance without an entry: it builds every slot of the scope.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    pub async fn open_default(self) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Everything).await
    }

    /// Opens the instance building nothing but its seeds: every other slot is built when
    /// something resolves it, so the instance costs only what it is used for.
    ///
    /// # Errors
    ///
    /// Fails when the parent belongs to another runtime, or a seed has no slot in the scope or
    /// shares one with another seed.
    pub async fn open_on_demand(self) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Nothing).await
    }

    /// Opens the instance to run `entry` as [`open`](Self::open) does, unless the entry's plan
    /// builds nothing: then the container opens the first time something resolves from the
    /// returned scope, with this builder's seeds. Until then, reads that never build see only
    /// the scopes it opens under, not the seeds.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open). Seeds and components are checked when the container opens.
    pub async fn open_when_needed(self, entry: &ScopeEntry<S>) -> crate::Result<EntryScope<S>> {
        let (view, parent) = self.target()?;
        let plans_nothing = view
            .scope_plan()
            .layouts
            .entry(entry.id)
            .is_some_and(EntryPlan::is_empty);

        if plans_nothing {
            return Ok(EntryScope::deferred(view, parent, entry.id, self.seeds));
        }

        let container = open_in(view, S::ID, parent, Builds::Entry(entry.id), self.seeds).await?;

        Ok(EntryScope::from_container(container))
    }

    async fn open_with(self, builds: Builds) -> crate::Result<OpenScope<S>> {
        let (view, parent) = self.target()?;
        let container = open_in(view, S::ID, parent, builds, self.seeds).await?;

        Ok(OpenScope {
            container,
            scope: PhantomData,
        })
    }

    /// The generation the instance opens on and the container it opens under.
    fn target(&self) -> crate::Result<(RuntimeView, Arc<ScopeContainer>)> {
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

        Ok((view, parent))
    }
}

/// Builds an instance of `scope` under `parent` from the layout `view` prepared for it, with
/// the slots `builds` names.
async fn open_in(
    view: RuntimeView,
    scope: ScopeId,
    parent: Arc<ScopeContainer>,
    builds: Builds,
    seeds: Vec<BoxedComponent>,
) -> crate::Result<Arc<ScopeContainer>> {
    let layouts = &view.scope_plan().layouts;
    let layout = layouts
        .get(&scope)
        .ok_or(crate::Error::UndeclaredScopeOpen { scope })?;
    let nothing;
    let plan = match builds {
        Builds::Entry(entry) => layouts.entry(entry),
        Builds::Everything => None,
        Builds::Nothing => {
            nothing = EntryPlan::nothing(layout);

            Some(&nothing)
        }
    };

    ScopeContainer::open_planned(
        parent,
        Arc::clone(view.scopes()),
        layout,
        plan,
        seeds,
        view.generation_state(),
    )
    .await
    .map_err(crate::Error::from)
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
