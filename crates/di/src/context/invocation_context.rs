//! The task-local [`InvocationContext`] and the hooks that establish, capture and bind it.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::task::futures::TaskLocalFuture;
use upwell_core::ScopeKey;

use super::{ContextError, ContextRuntime, TargetRef};
use crate::container::ScopeContainer;

tokio::task_local! {
    static CONTEXT: Option<InvocationContext>;
}

/// A future running inside an entered invocation context, or with every context masked.
#[doc(hidden)]
pub type Entered<F> = TaskLocalFuture<Option<InvocationContext>, F>;

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
    claim: Claim,
    opened: OnceLock<Arc<ScopeContainer>>,
}

/// Which opening of its scope a [`PendingScope`] binds.
#[derive(Clone, Copy)]
enum Claim {
    /// Any instance of the scope that opens under the parent, through [`bind_opened_scope`].
    AnyOpening,
    /// None: the code that made the context opens the instance and runs it in a context of
    /// its own, so an unrelated instance opened while it constructs cannot take its place.
    Owner,
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
        self.pending(scope, parent, Claim::AnyOpening)
    }

    /// This context running while the caller opens an instance of `scope` under `parent`, as
    /// [`with_pending_scope`](Self::with_pending_scope) does, except that no opening binds it:
    /// the caller runs the opened instance in [`with_scope`](Self::with_scope) itself. An
    /// instance of the same scope opened lazily while the caller's instance constructs then
    /// cannot stand in for it.
    #[doc(hidden)]
    pub fn with_opening_scope(self, scope: ScopeKey, parent: Arc<ScopeContainer>) -> Self {
        self.pending(scope, Some(parent), Claim::Owner)
    }

    /// This context waiting for an instance of `scope` under `parent` that `claim` binds.
    fn pending(self, scope: ScopeKey, parent: Option<Arc<ScopeContainer>>, claim: Claim) -> Self {
        Self {
            scope: ActiveScope::Pending(PendingScope {
                scope,
                parent,
                claim,
                opened: OnceLock::new(),
            }),
            ..self
        }
    }

    /// Runs `future` inside this context, on top of any context the task already runs in.
    #[doc(hidden)]
    pub fn enter<F: Future>(self, future: F) -> Entered<F> {
        CONTEXT.scope(Some(self), future)
    }

    /// Runs the synchronous `body` inside this context, on top of any context the thread
    /// already runs in, on whichever thread calls it. For captured work on a blocking or OS
    /// thread.
    #[doc(hidden)]
    pub fn enter_sync<R>(self, body: impl FnOnce() -> R) -> R {
        CONTEXT.sync_scope(Some(self), body)
    }

    /// Runs `future` inside `context`, or with any context the task runs in masked when it is
    /// `None`, so work captured outside every invocation never sees one it happens to run in.
    pub(crate) fn enter_or_mask<F: Future>(context: Option<Self>, future: F) -> Entered<F> {
        CONTEXT.scope(context, future)
    }

    /// Runs `body` inside `context`, or with any context the thread runs in masked when it is
    /// `None`, restoring the thread's context afterwards.
    pub(crate) fn enter_sync_or_mask<R>(context: Option<Self>, body: impl FnOnce() -> R) -> R {
        CONTEXT.sync_scope(context, body)
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
            .try_with(|context| context.as_ref().map(read))
            .ok()
            .flatten()
            .ok_or(ContextError::OutsideContext)
    }

    /// The innermost open scope instance: the whole chain up to the root.
    pub(crate) fn scope(&self) -> Result<Arc<ScopeContainer>, ContextError> {
        self.innermost()
            .map(Arc::clone)
            .ok_or(ContextError::OutsideInvocationScope)
    }

    /// The innermost open scope instance, borrowed.
    fn innermost(&self) -> Option<&Arc<ScopeContainer>> {
        match &self.scope {
            ActiveScope::None => None,
            ActiveScope::Open(scope) => Some(scope),
            ActiveScope::Pending(pending) => pending.opened.get().or(pending.parent.as_ref()),
        }
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

/// What the calling task or thread runs inside of, as [`with_context_state`] reads it.
#[doc(hidden)]
pub enum ContextState<'a> {
    /// No invocation context was ever entered on this task or thread.
    Absent,
    /// Work captured outside every invocation masks the context it runs in.
    Masked,
    /// An invocation context is entered: its open scope instances, innermost first.
    Entered(ScopeChain<'a>),
}

/// Open scope instances from one instance up to the root, innermost first.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct ScopeChain<'a> {
    next: Option<&'a Arc<ScopeContainer>>,
}

impl<'a> ScopeChain<'a> {
    /// `container` and every instance it opened under.
    pub fn starting_at(container: &'a Arc<ScopeContainer>) -> Self {
        Self {
            next: Some(container),
        }
    }

    /// The innermost instance, when there is one.
    pub fn innermost(&self) -> Option<&'a Arc<ScopeContainer>> {
        self.next
    }
}

impl<'a> Iterator for ScopeChain<'a> {
    type Item = &'a Arc<ScopeContainer>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;

        self.next = current.parent();

        Some(current)
    }
}

/// Runs `read` over what the calling task or thread runs inside of, borrowing the context
/// instead of cloning it. `read` must not enter or mask a context itself.
#[doc(hidden)]
pub fn with_context_state<R>(read: impl FnOnce(ContextState<'_>) -> R) -> R {
    if CONTEXT.try_with(|_| ()).is_err() {
        return read(ContextState::Absent);
    }

    CONTEXT.with(|context| match context {
        None => read(ContextState::Masked),
        Some(context) => read(ContextState::Entered(ScopeChain {
            next: context.innermost(),
        })),
    })
}

/// Binds `container`, just opened, into the running context when that context waits for an
/// instance of its scope under its parent. Returns whether it bound.
///
/// A protocol calls it from the task that opened the scope lazily; anywhere else it does
/// nothing. A context made by [`InvocationContext::with_opening_scope`] never binds.
#[doc(hidden)]
pub fn bind_opened_scope(container: &Arc<ScopeContainer>) -> bool {
    CONTEXT
        .try_with(
            |context| match context.as_ref().map(|context| &context.scope) {
                Some(ActiveScope::Pending(pending)) if pending.expects(container) => {
                    pending.opened.set(Arc::clone(container)).is_ok()
                }
                _ => false,
            },
        )
        .unwrap_or(false)
}

impl PendingScope {
    fn expects(&self, container: &ScopeContainer) -> bool {
        if matches!(self.claim, Claim::Owner) {
            return false;
        }

        let parent_matches = match (&self.parent, container.parent()) {
            (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
            (Some(_), None) => false,
            (None, _) => true,
        };

        self.scope.id() == container.scope().id() && parent_matches
    }
}
