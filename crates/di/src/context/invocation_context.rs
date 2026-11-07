//! The task-local [`InvocationContext`] and the hooks that establish, capture and bind it.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::task::futures::TaskLocalFuture;
use upwell_core::ScopeKey;

use super::{ContextError, ContextRuntime, TargetRef};
use crate::container::ScopeContainer;

tokio::task_local! {
    static CONTEXT: InvocationContext;
}

/// What one framework invocation runs inside of: its metadata, its runtime and its innermost
/// open scope instance. See the [module documentation](super).
#[must_use = "an invocation context does nothing until something enters it"]
#[derive(Clone)]
pub struct InvocationContext {
    target: TargetRef,
    /// Read only by [`root`](Self::root), which nothing but the test inspection helpers calls
    /// until the scope access lookups use it.
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    runtime: Arc<dyn ContextRuntime>,
    scope: ActiveScope,
}

/// The scope instance an [`InvocationContext`] runs in, if any.
#[derive(Clone)]
enum ActiveScope {
    /// No scope instance, such as in a job run or a lifecycle hook.
    None,
    /// An instance already open when the context was established.
    Open(Arc<ScopeContainer>),
    /// An instance that opens after the context was established.
    Pending(PendingScope),
}

/// A scope instance an [`InvocationContext`] expects to open later.
#[derive(Clone)]
struct PendingScope {
    scope: ScopeKey,
    parent: Option<Arc<ScopeContainer>>,
    opened: OnceLock<Arc<ScopeContainer>>,
}

impl InvocationContext {
    /// A context for `target` on `runtime`, with no scope instance.
    #[doc(hidden)]
    pub fn new(target: TargetRef, runtime: Arc<dyn ContextRuntime>) -> Self {
        Self {
            target,
            runtime,
            scope: ActiveScope::None,
        }
    }

    /// This context running in the open scope instance `scope`.
    #[doc(hidden)]
    pub fn with_scope(self, scope: Arc<ScopeContainer>) -> Self {
        Self {
            scope: ActiveScope::Open(scope),
            ..self
        }
    }

    /// This context running in an instance of `scope` that opens later, under `parent` when it
    /// has one. Until it opens, `parent` is the innermost scope instance.
    #[doc(hidden)]
    pub fn with_pending_scope(self, scope: ScopeKey, parent: Option<Arc<ScopeContainer>>) -> Self {
        Self {
            scope: ActiveScope::Pending(PendingScope {
                scope,
                parent,
                opened: OnceLock::new(),
            }),
            ..self
        }
    }

    /// Runs `future` inside this context, on top of any context the task already runs in.
    #[doc(hidden)]
    pub fn enter<F: Future>(self, future: F) -> TaskLocalFuture<Self, F> {
        CONTEXT.scope(self, future)
    }

    /// Runs the synchronous `body` inside this context, on top of any context the thread
    /// already runs in, on whichever thread calls it. For captured work on a blocking or OS
    /// thread.
    #[doc(hidden)]
    pub fn enter_sync<R>(self, body: impl FnOnce() -> R) -> R {
        CONTEXT.sync_scope(self, body)
    }

    /// A copy of the active context, to [`enter`](Self::enter) on another task. It holds the
    /// context's scope instance strongly.
    ///
    /// # Errors
    ///
    /// [`ContextError::OutsideContext`] when no context is active.
    #[doc(hidden)]
    pub fn capture() -> Result<Self, ContextError> {
        Self::with_current(Self::clone)
    }

    /// Runs `read` over the active context.
    pub(crate) fn with_current<R>(read: impl FnOnce(&Self) -> R) -> Result<R, ContextError> {
        CONTEXT
            .try_with(read)
            .map_err(|_| ContextError::OutsideContext)
    }

    /// The innermost open scope instance: the whole chain up to the root.
    pub(crate) fn scope(&self) -> Result<Arc<ScopeContainer>, ContextError> {
        let innermost = match &self.scope {
            ActiveScope::None => None,
            ActiveScope::Open(scope) => Some(scope),
            ActiveScope::Pending(pending) => pending.opened.get().or(pending.parent.as_ref()),
        };

        innermost
            .map(Arc::clone)
            .ok_or(ContextError::OutsideInvocationScope)
    }

    /// The scope this context waits for while it has not opened.
    pub(crate) fn unopened_scope(&self) -> Option<ScopeKey> {
        match &self.scope {
            ActiveScope::Pending(pending) if pending.opened.get().is_none() => Some(pending.scope),
            ActiveScope::None | ActiveScope::Open(_) | ActiveScope::Pending(_) => None,
        }
    }

    /// The root this context resolves singletons from: the root of its scope chain, pinned
    /// with the generation the chain runs on, or the runtime's current root without a scope.
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    pub(crate) fn root(&self) -> Result<Arc<ScopeContainer>, ContextError> {
        match self.scope() {
            Ok(scope) => scope.root_handle(),
            Err(_) => self.runtime.root(),
        }
        .ok_or(ContextError::RuntimeUnavailable)
    }
}

/// Inspection for tests, behind the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
impl InvocationContext {
    /// What this context's entry point is.
    pub fn target(&self) -> &TargetRef {
        &self.target
    }

    /// The scope of the innermost open instance, if any.
    pub fn scope_key(&self) -> Option<ScopeKey> {
        self.scope().ok().map(|scope| scope.scope())
    }

    /// Whether `container` is this context's innermost open scope instance.
    pub fn is_scope(&self, container: &Arc<ScopeContainer>) -> bool {
        self.scope()
            .is_ok_and(|scope| Arc::ptr_eq(&scope, container))
    }

    /// Whether this context and `other` run in the same innermost scope instance.
    pub fn same_scope(&self, other: &Self) -> bool {
        match (self.scope(), other.scope()) {
            (Ok(own), Ok(other)) => Arc::ptr_eq(&own, &other),
            _ => false,
        }
    }

    /// Whether this context resolves singletons from `root`.
    pub fn resolves_root(&self, root: &Arc<ScopeContainer>) -> bool {
        self.root().is_ok_and(|own| Arc::ptr_eq(&own, root))
    }
}

impl fmt::Debug for InvocationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvocationContext")
            .field("target", &self.target)
            .field("scope", &self.scope().ok().map(|scope| scope.scope()))
            .finish_non_exhaustive()
    }
}

impl ContextRuntime for ScopeContainer {
    fn root(&self) -> Option<Arc<ScopeContainer>> {
        self.root_handle()
    }
}

/// Binds `container`, just opened, into the running context when that context waits for an
/// instance of its scope under its parent. Returns whether it bound.
///
/// A protocol calls it from the task that opened the scope lazily; anywhere else it does
/// nothing.
#[doc(hidden)]
pub fn bind_opened_scope(container: &Arc<ScopeContainer>) -> bool {
    CONTEXT
        .try_with(|context| match &context.scope {
            ActiveScope::Pending(pending) if pending.expects(container) => {
                pending.opened.set(Arc::clone(container)).is_ok()
            }
            _ => false,
        })
        .unwrap_or(false)
}

impl PendingScope {
    fn expects(&self, container: &ScopeContainer) -> bool {
        let parent_matches = match (&self.parent, container.parent()) {
            (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
            (Some(_), None) => false,
            (None, _) => true,
        };

        self.scope.id() == container.scope().id() && parent_matches
    }
}
