//! The invocation targets the application runtime itself establishes.

use upwell_core::ScopeId;
use upwell_di::context::InvocationTarget;

/// Work run in a fresh instance of a scope by [`ScopeBuilder::run`](crate::ScopeBuilder::run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeRunTarget {
    /// The scope the work runs in.
    pub scope: ScopeId,
}

impl InvocationTarget for ScopeRunTarget {}

/// A `'static` [`TargetRef`](upwell_di::context::TargetRef) for work run in `$scope`, a constant expression that may depend on
/// generic parameters. Allocates nothing.
macro_rules! scope_run_target {
    ($scope:expr) => {
        ::upwell_di::context::TargetRef::from_static(
            const { &$crate::runtime::ScopeRunTarget { scope: $scope } },
        )
    };
}

pub(crate) use scope_run_target;

/// One firing of a lifecycle, reload, transition or retirement hook kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookTarget {
    /// The hook kind's name, such as `startup`.
    pub kind: &'static str,
}

impl InvocationTarget for HookTarget {}

/// A `'static` [`TargetRef`](upwell_di::context::TargetRef) for a firing of the hook kind named `$kind`, a constant expression
/// that may depend on generic parameters. Allocates nothing.
macro_rules! hook_target {
    ($kind:expr) => {
        ::upwell_di::context::TargetRef::from_static(
            const { &$crate::runtime::HookTarget { kind: $kind } },
        )
    };
}

pub(crate) use hook_target;
