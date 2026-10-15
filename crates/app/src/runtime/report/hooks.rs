use upwell_hooks::HookRole;

use super::TransitionPhase;

/// One transition hook that ran during a pre-commit phase, in firing order.
///
/// Participants appear in registration order, up to and including one that ended the
/// attempt; observers follow once every participant accepted. Carries the owning
/// component's stable ID and redacted categories only, never a hook's error message.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct TransitionHookReport {
    /// The pre-commit phase the hook ran in.
    pub phase: TransitionPhase,
    /// The stable ID of the component owning the hook, matching its
    /// [`ComponentTransitionReport::component`](crate::ComponentTransitionReport::component)
    /// when the transition affects it.
    pub component: &'static str,
    /// Whether the hook could affect the attempt.
    #[cfg_attr(feature = "serde", serde(serialize_with = "super::serialize::role"))]
    pub role: HookRole,
    /// How the hook ended.
    pub outcome: TransitionHookOutcome,
}

/// How one transition hook ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(tag = "outcome", rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum TransitionHookOutcome {
    /// A participant accepted, or an observer completed.
    Completed,
    /// A participant rejected the attempt with the given author-chosen reason code.
    Rejected {
        /// The static reason code the hook returned.
        reason: &'static str,
    },
    /// The hook returned an error or could not run. Fail-closed for a participant,
    /// reported only for an observer.
    Failed,
    /// The hook panicked. The payload is never recorded.
    Panicked,
}
