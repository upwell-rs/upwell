//! A parent chosen at run time for scope `S`, whose own scope type was checked when it was made.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::Scope;
use upwell_di::ScopeContainer;

use super::{OpenScope, OpensUnder};

/// An open instance that scope `S` may open under, with the instance's own scope type erased.
///
/// [`ScopeBuilder::under`](super::ScopeBuilder::under) needs the parent's scope type at
/// compile time. Code that picks a parent per opening, such as a request opened under the
/// caller's session when it has one, keeps a `ParentScope<S>` instead: making one checks
/// `S: OpensUnder<P>`, and [`ScopeBuilder::under_parent`](super::ScopeBuilder::under_parent)
/// opens under it. The instance stays open while the parent is held.
pub struct ParentScope<S: Scope> {
    container: Arc<ScopeContainer>,
    child: PhantomData<fn() -> S>,
}

impl<S: Scope> ParentScope<S> {
    /// `parent`, as a parent of scope `S`.
    pub fn new<P: Scope>(parent: &OpenScope<P>) -> Self
    where
        S: OpensUnder<P>,
    {
        Self {
            container: Arc::clone(parent.container()),
            child: PhantomData,
        }
    }

    /// The parent instance's container.
    pub fn container(&self) -> &Arc<ScopeContainer> {
        &self.container
    }
}

impl<S: Scope> Clone for ParentScope<S> {
    fn clone(&self) -> Self {
        Self {
            container: Arc::clone(&self.container),
            child: PhantomData,
        }
    }
}

impl<S: Scope> fmt::Debug for ParentScope<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParentScope")
            .field("child", &S::NAME)
            .field("parent", &self.container.scope().name())
            .finish()
    }
}

impl<P: Scope, S: OpensUnder<P>> From<&OpenScope<P>> for ParentScope<S> {
    fn from(parent: &OpenScope<P>) -> Self {
        Self::new(parent)
    }
}
