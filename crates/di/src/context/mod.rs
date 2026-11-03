//! The invocation context: what the current task runs inside of.
//!
//! Every framework entry point (an HTTP request, a WebSocket connection or message, an RPC
//! call, a job run, a lifecycle hook) runs its whole future inside one [`InvocationContext`],
//! kept in a task-local. It holds:
//!
//! - the [`Invocation`] metadata saying which entry point this is;
//! - the application runtime, opaque here ([`ContextRuntime`]), which supplies the root of the
//!   current generation while no scope is open;
//! - the innermost open scope instance. A [`ScopeContainer`] links to its parents, so the
//!   innermost instance is the whole chain up to the root, and it pins the runtime generation
//!   it was opened on.
//!
//! Reading the context is framework-internal: the lookups are `pub(crate)`, and other
//! framework crates only establish a context ([`InvocationContext::enter`]), capture it to
//! re-enter elsewhere ([`InvocationContext::capture`]), or bind a lazily opened scope
//! ([`bind_opened_scope`]). Users reach scoped values through declared dependencies only.
//! Outside every entry point a lookup fails with [`ContextError::OutsideContext`], never a
//! panic.
//!
//! # Scopes that open after the context
//!
//! A protocol may open an invocation's scope only when something first resolves from it: HTTP
//! opens the request scope on the first injection, RPC and WebSocket open a call or message
//! scope whose entry builds nothing on its first resolution. The context is established
//! before that, around the whole dispatch, with a *pending* scope: the scope it expects and,
//! when the scope opens under one, the parent instance. Until the scope opens, the parent is
//! the innermost scope; the opening then binds its container into the pending slot of the
//! running context through [`bind_opened_scope`].
//!
//! The slot is a once-cell stored inline in the context, which the task-local holds by value,
//! so establishing a context allocates nothing. Only code running in the dispatching task
//! sees the bind. A context captured before the scope opened keeps the pending state as it
//! was at the capture, so work it is re-entered in sees the scope only if it opens the scope
//! itself; capture after the scope opens to carry it.
//!
//! # Capture and re-entry
//!
//! [`InvocationContext::capture`] clones the active context: a few reference-count bumps and
//! no allocation. The clone holds its scope instance strongly, so the instance lives until the
//! clone is dropped. [`InvocationContext::enter`] runs a future inside it, on any task.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::task::futures::TaskLocalFuture;
use upwell_core::{ScopeId, ScopeKey};

use crate::container::ScopeContainer;

mod invocation;

pub use invocation::Invocation;

tokio::task_local! {
    static CONTEXT: InvocationContext;
}

/// Why the active invocation context cannot answer a lookup.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    /// No framework entry point established a context for the running task.
    #[error("no invocation context is active: the code runs outside every framework entry point")]
    OutsideContext,
    /// The context has no open scope instance, such as in a job run or a lifecycle hook.
    #[error("the active invocation context has no open scope instance")]
    OutsideInvocationScope,
    /// The context's runtime no longer has a root, because the application shut down.
    #[error("the active invocation context's runtime has no root")]
    RuntimeUnavailable,
}

/// The application runtime an invocation context belongs to, opaque to the DI layer.
#[doc(hidden)]
pub trait ContextRuntime: Any + Send + Sync {
    /// The root of the generation a context without an open scope resolves from.
    fn root(&self) -> Option<Arc<ScopeContainer>>;
}

/// What one framework invocation runs inside of: its metadata, its runtime and its innermost
/// open scope instance. See the [module documentation](self).
#[derive(Clone)]
pub struct InvocationContext {
    invocation: Invocation,
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
    scope: ScopeId,
    parent: Option<Arc<ScopeContainer>>,
    opened: OnceLock<Arc<ScopeContainer>>,
}

impl InvocationContext {
    /// A context for `invocation` on `runtime`, with no scope instance.
    #[doc(hidden)]
    pub fn new(invocation: Invocation, runtime: Arc<dyn ContextRuntime>) -> Self {
        Self {
            invocation,
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
                scope: scope.id(),
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

    /// What this context's entry point is.
    #[doc(hidden)]
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    /// The scope of the innermost open instance, if any.
    #[doc(hidden)]
    pub fn scope_key(&self) -> Option<ScopeKey> {
        self.scope().ok().map(|scope| scope.scope())
    }

    /// Whether `container` is this context's innermost open scope instance.
    #[doc(hidden)]
    pub fn is_scope(&self, container: &Arc<ScopeContainer>) -> bool {
        self.scope()
            .is_ok_and(|scope| Arc::ptr_eq(&scope, container))
    }

    /// Whether this context resolves singletons from `root`.
    #[doc(hidden)]
    pub fn resolves_root(&self, root: &Arc<ScopeContainer>) -> bool {
        self.root().is_ok_and(|own| Arc::ptr_eq(&own, root))
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

    /// The root this context resolves singletons from: the root of its scope chain, pinned
    /// with the generation the chain runs on, or the runtime's current root without a scope.
    pub(crate) fn root(&self) -> Result<Arc<ScopeContainer>, ContextError> {
        match self.scope() {
            Ok(scope) => scope.root_handle(),
            Err(_) => self.runtime.root(),
        }
        .ok_or(ContextError::RuntimeUnavailable)
    }
}

impl fmt::Debug for InvocationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvocationContext")
            .field("invocation", &self.invocation)
            .field("scope", &self.scope_key())
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

        self.scope == container.scope().id() && parent_matches
    }
}

#[cfg(test)]
mod tests;
