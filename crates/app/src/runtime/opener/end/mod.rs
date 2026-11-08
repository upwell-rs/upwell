//! The end of a scope instance: [`ScopeEnded<S>`] tells the singleton observers of scope `S`
//! that an instance of it ended, and [`AnyScopeEnded`] tells those of every scope.
//!
//! The hook is observer-only. It sees the instance's identity, how it ended and its loggable
//! params, never the container, its components or any secret, so it cannot veto, resurrect or
//! mutate the instance, and it never delays the code that ended it.
//!
//! # Firing
//!
//! An instance ends once, when the code that owned it lets go: a [`ScopeRun`](crate::ScopeRun)
//! when its work finishes, fails or is dropped, and an [`OpenScope`](crate::OpenScope) or
//! [`EntryScope`](crate::EntryScope) when its last handle is dropped, which is how a protocol
//! ends a request or connection scope. The owner's container reference is released first, so
//! the instance's components drop with it unless something else shares them.
//!
//! The hooks then run on a task spawned onto the ambient Tokio runtime, never inline, and the
//! task owns the instance's log until they finish. They therefore run after the instance's work
//! and before its log closes: the events a hook makes inside the instance's span are still
//! captured, and a stream of the instance's log ends only afterwards. The task pins the
//! generation the instance ran on while it runs, and holds nothing else of the instance.
//!
//! Dispatch is by type. The kind carries its scope as a type argument, so
//! `#[hook(ScopeEnded<Session>)]` registers under the kind `ScopeEnded<Session>` and an instance
//! of `Session` fires exactly that kind, with no filtering of other scopes' hooks. Both kinds
//! fire for the same instance, [`ScopeEnded<S>`] first. Whether either applies is decided when
//! the instance opens by two lookups in the generation's hook index, and an instance nothing
//! observes allocates no end at all.
//!
//! # Failures
//!
//! A failing or panicking hook is traced as `scope end hook failed` on `upwell::scope`, with
//! the scope, how it ended, the hook owner and the failure category but never its error message
//! or panic payload. It never affects the code that ended the instance or other hooks. With no
//! Tokio runtime to spawn onto, the end is traced as skipped and no hook runs.

mod guard;

use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{DependencyDescriptor, Scope, ScopeId};
use upwell_hooks::{HookKind, HookParam, HookParticipation};

use super::super::transition_hooks::event_dependency;
use crate::logs::LogField;

pub(super) use guard::{InstanceEnd, Observed};

/// Fired once on each singleton hook declared for scope `S` when an instance of `S` ends, with
/// the instance's identity as the parameter: `#[hook(ScopeEnded<Session>)]`.
pub struct ScopeEnded<S: Scope>(PhantomData<fn() -> S>);

impl<S: Scope> HookKind for ScopeEnded<S> {
    type Output = ();
    type Cx = ScopeEndContext;

    const NAME: &'static str = "scope_ended";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

/// Fired once on each singleton hook when an instance of any scope ends, with the instance's
/// identity as the parameter. The hook reads which scope from [`ScopeEnd::scope`]; hook
/// [`ScopeEnded<S>`] to observe one scope only.
pub struct AnyScopeEnded;

impl HookKind for AnyScopeEnded {
    type Output = ();
    type Cx = ScopeEndContext;

    const NAME: &'static str = "any_scope_ended";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

/// How a scope instance ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ScopeEndKind {
    /// A run's work finished and its targets resolved, whatever the work itself returned: an
    /// `Err` in the work's own output is a completed run.
    Completed,
    /// A run's instance could not open or its work's targets could not resolve, a failure of
    /// the framework rather than of the work.
    Failed,
    /// A run was dropped before it finished, by its caller or by unwinding.
    Cancelled,
    /// The last handle to an instance a caller opened, such as a protocol's request scope,
    /// was dropped. The owner does not say how its work went.
    Released,
}

impl ScopeEndKind {
    /// The kind's stable snake-case name, as used in tracing.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Released => "released",
        }
    }
}

/// One scope instance's end: which scope, how it ended and its loggable params. A
/// [`ScopeEnded`] hook parameter.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ScopeEnd {
    /// The ended instance's scope.
    pub scope: ScopeId,
    /// The scope's display name.
    pub name: &'static str,
    /// How the instance ended.
    pub kind: ScopeEndKind,
    /// The instance's loggable params, the fields its scope marks `#[log]`, rendered when the
    /// instance opened. Never anything else the instance held.
    pub params: Arc<[LogField]>,
}

/// The context a [`ScopeEnded`] hook's parameters are extracted from: one instance's end.
pub struct ScopeEndContext {
    end: ScopeEnd,
}

impl ScopeEndContext {
    pub(crate) fn new(end: ScopeEnd) -> Self {
        Self { end }
    }
}

impl<S: Scope> HookParam<ScopeEnded<S>> for ScopeEnd {
    fn dependency(_path: Option<&'static str>) -> DependencyDescriptor {
        event_dependency::<Self>("ScopeEnd")
    }

    fn extract(cx: &ScopeEndContext, _path: Option<&'static str>) -> upwell_hooks::Result<Self> {
        Ok(cx.end.clone())
    }
}

impl HookParam<AnyScopeEnded> for ScopeEnd {
    fn dependency(_path: Option<&'static str>) -> DependencyDescriptor {
        event_dependency::<Self>("ScopeEnd")
    }

    fn extract(cx: &ScopeEndContext, _path: Option<&'static str>) -> upwell_hooks::Result<Self> {
        Ok(cx.end.clone())
    }
}
