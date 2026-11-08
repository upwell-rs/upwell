//! Opening scope instances through the prepared [`ScopeOpener`] protocols and users share.
//!
//! An opener checks once, when it is created, that its scope is declared, and resolves each
//! entry it opens for once. Opening an instance then pins a runtime generation, takes the
//! scope's prepared layout and builds the container. The parent's place in the topology is
//! checked only in debug builds; each seed is checked against the slot it fills.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{EntryKey, Scope, ScopeId};
use upwell_di::ScopeContainer;
use upwell_di::context::{ContextRuntime, InvocationContext, TargetRef};

use super::{AppRuntime, RuntimeGeneration, RuntimeView};
use crate::builtins::RuntimeReloader;
use crate::entry::EntryId;
use crate::logs::{ScopeLogSink, ScopeLogStream};
use tracing::Span;

mod builder;
mod end;
mod entry_scope;
mod inject;
mod log_options;
mod run;
mod run_error;
mod work;

use builder::{Builds, open_in};
use end::{InstanceEnd, Observed};

pub use builder::{Opening, ScopeBuilder};
pub use end::{AnyScopeEnded, ScopeEnd, ScopeEndContext, ScopeEndKind, ScopeEnded};
pub use entry_scope::EntryScope;

pub use run::ScopeRun;
pub use run_error::RunError;
pub use work::{ScopeWork, TryScopeWork};

/// Declares that scope `Self` may open under scope `P`.
///
/// Implement it for every parent the scope's topology lists, with [`Singleton`] standing for
/// the application root. [`ScopeBuilder::under`] requires it, so opening a scope under a
/// parent its topology does not list fails to compile.
pub trait OpensUnder<P: Scope>: Scope {}

/// The prepared opener of scope `S`, shared by protocols and user code.
///
/// Created once with [`AppRuntime::opener`], which checks that `S` is declared, or injected as
/// a parameter (an `#[init]` constructor, a job, an entry or [`ScopeBuilder::run`] work).
/// Injected, it grants permission to open `S`, not to look anything up in it, and holds its
/// runtime weakly, so a singleton keeping it never keeps the runtime alive. Cloning it is
/// cheap: an opener made by [`AppRuntime::opener`] bumps one reference count.
pub struct ScopeOpener<S: Scope> {
    runtime: OpenerRuntime,
    scope: PhantomData<fn() -> S>,
}

/// The runtime a [`ScopeOpener`] opens instances on.
#[derive(Clone)]
enum OpenerRuntime {
    /// Held by the code that prepared the opener, such as a protocol's serve loop.
    Owned(Arc<AppRuntime>),
    /// Held weakly by an injected opener through the runtime's own reloader, which is attached
    /// once the runtime is built: the runtime's components may keep the opener.
    Injected(RuntimeReloader),
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
    sink: Option<Arc<ScopeLogSink>>,
    span: Span,
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
        self.declares::<S>()?;

        Ok(ScopeOpener {
            runtime: OpenerRuntime::Owned(Arc::new(self.clone())),
            scope: PhantomData,
        })
    }

    /// Fails with [`UndeclaredScopeOpen`](crate::Error::UndeclaredScopeOpen) unless the
    /// topology declares `S`.
    fn declares<S: Scope>(&self) -> crate::Result<()> {
        if self.scope_topology().boundary(&S::ID).is_none() {
            return Err(crate::Error::UndeclaredScopeOpen { scope: S::ID });
        }

        Ok(())
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

impl OpenerRuntime {
    /// The runtime instances of `S` open on, upgraded for the call when the opener holds it
    /// weakly. An injected opener checks that the runtime declares `S` here, since it may be
    /// injected before the runtime exists.
    fn get<S: Scope>(&self) -> crate::Result<Cow<'_, AppRuntime>> {
        match self {
            Self::Owned(runtime) => Ok(Cow::Borrowed(runtime)),
            Self::Injected(reloader) => {
                let runtime = reloader
                    .runtime()
                    .and_then(|runtime| runtime.upgrade())
                    .ok_or(crate::Error::RuntimeUnavailable)?;

                runtime.declares::<S>()?;

                Ok(Cow::Owned(runtime))
            }
        }
    }
}

impl<S: Scope> ScopeOpener<S> {
    /// The entry with stable identity `key`, resolved once for every later opening.
    ///
    /// # Errors
    ///
    /// [`UnknownEntry`](crate::Error::UnknownEntry) when no entry has `key`,
    /// [`EntryScopeMismatch`](crate::Error::EntryScopeMismatch) when it runs in another scope,
    /// and [`RuntimeUnavailable`](crate::Error::RuntimeUnavailable) when an injected opener
    /// outlives its runtime.
    pub fn entry(&self, key: EntryKey) -> crate::Result<ScopeEntry<S>> {
        let runtime = self.runtime.get::<S>()?;
        let entries = &runtime.entries;
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
    /// to a scoped component, so a reload does not change the answer. False once an injected
    /// opener outlives its runtime.
    pub fn builds_eagerly(&self) -> bool {
        self.runtime.get::<S>().is_ok_and(|runtime| {
            runtime
                .view()
                .scope_plan()
                .layouts
                .get(&S::ID)
                .is_some_and(|layout| layout.eager_components().next().is_some())
        })
    }

    /// Whether an opening of the scope on the current generation must open it at once: it
    /// builds an eager component, or an invocation-context accessor reads one of its slots.
    /// False once an injected opener outlives its runtime.
    pub fn opens_at_once(&self) -> bool {
        self.runtime.get::<S>().is_ok_and(|runtime| {
            runtime
                .view()
                .scope_plan()
                .layouts
                .get(&S::ID)
                .is_some_and(|layout| {
                    layout.eager_components().next().is_some() || layout.is_accessed()
                })
        })
    }

    /// Whether some generation may make an opening of the scope open at once: the scope
    /// builds eagerly, or a component the application registers, enabled or not, declares an
    /// invocation-context edge. A protocol installs its at-once opening only then, and checks
    /// [`opens_at_once`](Self::opens_at_once) per opening.
    pub fn may_open_at_once(&self) -> bool {
        self.builds_eagerly()
            || self.runtime.get::<S>().is_ok_and(|runtime| {
                runtime
                    .view()
                    .condition()
                    .catalog()
                    .components
                    .iter()
                    .any(declares_accessor)
            })
    }

    /// A context for `target` on this opener's runtime, with no scope instance yet. An
    /// opener a serve loop prepared shares its runtime handle, so this allocates nothing; an
    /// injected one allocates a weak handle. Until a scope instance joins it, it resolves the
    /// root of the runtime's current generation; the instance then pins its own generation.
    #[doc(hidden)]
    pub fn context(&self, target: TargetRef) -> InvocationContext {
        let runtime: Arc<dyn ContextRuntime> = match &self.runtime {
            OpenerRuntime::Owned(runtime) => Arc::clone(runtime) as Arc<dyn ContextRuntime>,
            OpenerRuntime::Injected(reloader) => Arc::new(reloader.clone()),
        };

        InvocationContext::new(target, runtime)
    }

    /// Starts an instance with the value `params` under the application root;
    /// [`ScopeBuilder::under`] picks another parent.
    ///
    /// Components in the instance inject `params` as `Arc<S>`. It is stored once per instance
    /// in the slot the scope's declaration adds, and a zero-sized scope stores nothing.
    pub fn builder(&self, params: S) -> ScopeBuilder<'_, S> {
        ScopeBuilder::new(self, params)
    }
}

impl<S: Scope> Clone for ScopeOpener<S> {
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
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
    /// An open instance with the span and the sink its container retains.
    fn new(container: Arc<ScopeContainer>, observed: Observed) -> Self {
        Self {
            container,
            sink: observed.sink,
            span: observed.span,
            scope: PhantomData,
        }
    }

    /// Whether the instance's container retains an end: a log sink or an end hook observes it.
    #[cfg(test)]
    pub(in crate::runtime) fn has_end(&self) -> bool {
        crate::logs::is_retained(&self.container)
    }

    /// The instance's `upwell.scope` span, to instrument the work that runs in it. Events made
    /// inside it reach the instance's log only when it was opened with
    /// [`capture_logs`](ScopeBuilder::capture_logs); the span is disabled when no subscriber
    /// enables it. Inside an unrelated invocation's context, events follow that invocation
    /// instead, unless the instance opened under it; see [`ScopeLogLayer`](crate::logs::ScopeLogLayer).
    pub fn span(&self) -> Span {
        self.span.clone()
    }

    /// A stream of the records the instance captures from now on, ending when the last handle
    /// to the instance goes. Already ended unless the instance was opened with
    /// [`capture_logs`](ScopeBuilder::capture_logs).
    pub fn logs(&self) -> ScopeLogStream {
        self.sink
            .as_ref()
            .map_or_else(ScopeLogStream::ended, |sink| sink.subscribe())
    }

    /// The instance's container.
    pub fn container(&self) -> &Arc<ScopeContainer> {
        &self.container
    }

    /// The instance's container, giving up the scope type. The container keeps the instance's
    /// log and end hooks alive, so the instance ends when its last owner lets go of the
    /// container, not when this handle does.
    pub fn into_container(self) -> Arc<ScopeContainer> {
        self.container
    }
}

impl<S: Scope> Clone for OpenScope<S> {
    fn clone(&self) -> Self {
        Self {
            container: Arc::clone(&self.container),
            sink: self.sink.clone(),
            span: self.span.clone(),
            scope: PhantomData,
        }
    }
}

/// Whether a factory of `component` declares an invocation-context edge.
fn declares_accessor(component: &upwell_di::ComponentDescriptor) -> bool {
    (component.factories)().iter().any(|factory| {
        (factory.dependencies)()
            .iter()
            .any(|dependency| dependency.resolution == upwell_core::ResolutionMode::Current)
    })
}
