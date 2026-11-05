//! The invocation targets the application runtime itself establishes.

use std::sync::Arc;

use upwell_core::ScopeId;
use upwell_di::context::{InvocationTarget, TargetRef};

/// Work run in a fresh instance of a scope by [`ScopeBuilder::run`](crate::ScopeBuilder::run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeRunTarget {
    /// The scope the work runs in.
    pub scope: ScopeId,
}

impl InvocationTarget for ScopeRunTarget {}

impl ScopeRunTarget {
    /// A shared target for work run in `scope`.
    pub fn target(scope: ScopeId) -> TargetRef {
        TargetRef::shared(Arc::new(Self { scope }))
    }
}

/// One firing of a lifecycle, reload, transition or retirement hook kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookTarget {
    /// The hook kind's name, such as `startup`.
    pub kind: &'static str,
}

impl InvocationTarget for HookTarget {}

impl HookTarget {
    /// A shared target for a firing of the hook kind named `kind`. Hooks are not a hot path, so
    /// one small allocation per firing keeps every kind, generic or not, on one constructor.
    pub fn target(kind: &'static str) -> TargetRef {
        TargetRef::shared(Arc::new(Self { kind }))
    }
}
