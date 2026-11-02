//! Opening scope instances through the prepared [`ScopeOpener`] protocols and users share.
//!
//! An opener checks once, when it is created, that its scope is declared, and resolves each
//! entry it opens for once. Opening an instance then pins a runtime generation, takes the
//! scope's prepared layout and builds the container. The parent's place in the topology is
//! checked only in debug builds; each seed is checked against the slot it fills.

use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{EntryKey, Scope, ScopeId};
use upwell_di::ScopeContainer;

use super::{AppRuntime, RuntimeGeneration, RuntimeView};
use crate::entry::EntryId;

mod builder;
mod entry_scope;

use builder::{Builds, open_in};

pub use builder::{Opening, ScopeBuilder};
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

/// An open instance of scope `S`, usable as a typed parent for [`ScopeBuilder::under`].
pub struct OpenScope<S: Scope> {
    container: Arc<ScopeContainer>,
    scope: PhantomData<fn() -> S>,
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

    /// Whether every opening of the scope builds an eager component. A condition cannot apply
    /// to a scoped component, so a reload does not change the answer.
    pub fn builds_eagerly(&self) -> bool {
        self.runtime
            .view()
            .scope_plan()
            .layouts
            .get(&S::ID)
            .is_some_and(|layout| layout.eager_components().next().is_some())
    }

    /// Starts an instance under the application root; [`ScopeBuilder::under`] picks another
    /// parent.
    pub fn builder(&self) -> ScopeBuilder<'_, S> {
        ScopeBuilder::new(self)
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
