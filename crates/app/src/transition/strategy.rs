//! Component-owned transition strategies and the restart reasons that reject them.
//!
//! The complete strategy contract — selection order, valid actions and scopes, failure,
//! panic and cancellation semantics — is documented in `docs/transition-strategies.md`.

use std::fmt;

use upwell_di::{EffectiveNodeRole, NodeAction, PlannedNode, ReasonKind, TransitionPlan};

use super::CandidateGraph;

/// A component's explicit choice for satisfying one planned singleton transition.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentTransitionStrategy {
    /// Preserve the current instance without invoking user construction code.
    Retain,
    /// Invoke the candidate descriptor's ordinary selected factory.
    Reconstruct,
    /// Invoke the candidate descriptor's static replacement factory with an immutable
    /// snapshot of the active instance it replaces.
    ///
    /// The replacement resolves every dependency from the candidate generation and may
    /// not consume or mutate the previous instance, so a later preparation failure
    /// leaves the active generation intact. Valid only for singleton `Replace` and
    /// `RebindLive` nodes, which have an active instance to read. The previous instance
    /// is read through the same generation-snapshot seam as `Retain`, so a descriptor
    /// without a snapshot adapter cannot be replaced from its previous instance.
    Replace,
    /// Let the candidate descriptor's transition handler select a one-shot factory for
    /// this planned change, falling back to `Replace` when the component declares a
    /// replacement factory and to `Reconstruct` otherwise when the handler declines.
    ///
    /// The one-shot factory may read the previous instance and may resolve only the
    /// dependencies the ordinary factory declares. Valid for singleton `Replace` and
    /// `RebindLive` nodes whose component declares a transition handler.
    Transition,
    /// Keep the active instance and update it in place through the candidate
    /// descriptor's explicit hot-update preparation.
    ///
    /// Preparation reads the instance and candidate dependencies before commit and
    /// never mutates it; the prepared action applies during the terminal publication,
    /// and a failed or rejected transition drops it unapplied. Valid for singleton
    /// `Replace` and `RebindLive` nodes whose component declares a hot update and whose
    /// identity, factory and dependency contract are unchanged.
    HotUpdate,
}

impl ComponentTransitionStrategy {
    /// Whether the active instance carries into the candidate generation.
    pub(crate) fn retains_instance(self) -> bool {
        matches!(self, Self::Retain | Self::HotUpdate)
    }

    /// Whether candidate construction reads a snapshot of the active instance.
    pub(crate) fn reads_previous(self) -> bool {
        matches!(self, Self::Replace | Self::Transition)
    }
}

/// One component-owned strategy decision for a transition attempt.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentTransitionDecision {
    pub component: &'static str,
    pub strategy: ComponentTransitionStrategy,
}

/// Why an otherwise valid graph transition requires process restart in the v1 runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum RestartReason {
    MissingDecision,
    DuplicateDecision,
    UnknownDecision,
    StaleRetention,
    GenerationBoundDependency,
    ManualInstanceUnsupported,
    HandleRetentionUnsupported,
    LiveRebindUnsupported,
    RemovalUnsupported,
    NonSingleton,
    FactoryUnavailable,
    ReplacementUnavailable,
    PreviousUnavailable,
    TransitionHandlerUnavailable,
    HotUpdateUnavailable,
    HotUpdateIncompatible,
}

impl RestartReason {
    /// The reason's stable snake-case name, as used in serialized reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingDecision => "missing_decision",
            Self::DuplicateDecision => "duplicate_decision",
            Self::UnknownDecision => "unknown_decision",
            Self::StaleRetention => "stale_retention",
            Self::GenerationBoundDependency => "generation_bound_dependency",
            Self::ManualInstanceUnsupported => "manual_instance_unsupported",
            Self::HandleRetentionUnsupported => "handle_retention_unsupported",
            Self::LiveRebindUnsupported => "live_rebind_unsupported",
            Self::RemovalUnsupported => "removal_unsupported",
            Self::NonSingleton => "non_singleton",
            Self::FactoryUnavailable => "factory_unavailable",
            Self::ReplacementUnavailable => "replacement_unavailable",
            Self::PreviousUnavailable => "previous_unavailable",
            Self::TransitionHandlerUnavailable => "transition_handler_unavailable",
            Self::HotUpdateUnavailable => "hot_update_unavailable",
            Self::HotUpdateIncompatible => "hot_update_incompatible",
        }
    }
}

impl fmt::Display for RestartReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::MissingDecision => "the component did not choose a transition strategy",
            Self::DuplicateDecision => "the component produced conflicting transition decisions",
            Self::UnknownDecision => "the decision does not belong to an affected component",
            Self::StaleRetention => "retaining the component would preserve a stale dependency",
            Self::GenerationBoundDependency => {
                "the component depends on generation-bound runtime state"
            }
            Self::ManualInstanceUnsupported => "the pre-built component has no transition contract",
            Self::HandleRetentionUnsupported => {
                "the component handle cannot be isolated across runtime generations"
            }
            Self::LiveRebindUnsupported => "a changed live dependency cannot be rebound in place",
            Self::RemovalUnsupported => "component removal is not integrated yet",
            Self::NonSingleton => "runtime transitions are singleton-only",
            Self::FactoryUnavailable => "the candidate component has no ordinary factory",
            Self::ReplacementUnavailable => {
                "the candidate component declares no replacement factory"
            }
            Self::PreviousUnavailable => "the component has no active instance to replace",
            Self::TransitionHandlerUnavailable => {
                "the candidate component declares no transition handler"
            }
            Self::HotUpdateUnavailable => "the candidate component declares no hot update",
            Self::HotUpdateIncompatible => {
                "the component's identity, factory or dependency contract changed, so its instance cannot update in place"
            }
        };

        formatter.write_str(reason)
    }
}

/// A validated transition that cannot be applied safely by the current runtime.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("component '{component}' requires restart: {reason}")]
pub struct RestartRequired {
    pub component: &'static str,
    pub required: Option<NodeAction>,
    pub reason: RestartReason,
}

/// Checks one component's strategy against the planner's minimum action for its node.
///
/// Retaining a `RebindLive` node is valid only when `plan` can rebind every one of its
/// changed live bindings in place.
pub(super) fn validate_decision(
    candidate: &CandidateGraph,
    plan: &TransitionPlan,
    node: &PlannedNode,
    strategy: Option<ComponentTransitionStrategy>,
) -> crate::Result<()> {
    use ComponentTransitionStrategy::{HotUpdate, Reconstruct, Replace, Retain, Transition};

    if node.role != EffectiveNodeRole::Singleton {
        return restart(node, RestartReason::NonSingleton);
    }

    match (node.action, strategy) {
        (NodeAction::Remove, _) => restart(node, RestartReason::RemovalUnsupported),
        (NodeAction::RebindLive, Some(Retain)) if plan.rebinds_in_place(node.component) => Ok(()),
        (NodeAction::RebindLive, Some(Retain)) => {
            restart(node, RestartReason::LiveRebindUnsupported)
        }
        (NodeAction::Replace, Some(Retain)) => restart(node, RestartReason::StaleRetention),
        (NodeAction::Retain, Some(Retain)) => Ok(()),
        (NodeAction::Add | NodeAction::Replace | NodeAction::RebindLive, Some(Reconstruct)) => {
            require_factory(candidate, node, strategy)
        }
        (NodeAction::Replace | NodeAction::RebindLive, Some(Replace | Transition)) => {
            require_factory(candidate, node, strategy)
        }
        (NodeAction::Replace | NodeAction::RebindLive, Some(HotUpdate))
            if !updates_in_place(node) =>
        {
            restart(node, RestartReason::HotUpdateIncompatible)
        }
        (NodeAction::Replace | NodeAction::RebindLive, Some(HotUpdate)) => {
            require_factory(candidate, node, strategy)
        }
        (NodeAction::Add, Some(Replace | Transition | HotUpdate)) => {
            restart(node, RestartReason::PreviousUnavailable)
        }
        _ => restart(node, RestartReason::MissingDecision),
    }
}

/// Whether `node`'s reasons leave its instance updatable in place: its identity,
/// factory and dependency contract are unchanged.
pub(super) fn updates_in_place(node: &PlannedNode) -> bool {
    node.reasons.iter().all(|reason| {
        !matches!(
            reason.kind,
            ReasonKind::FactoryChanged
                | ReasonKind::IdentityChanged
                | ReasonKind::DependencyContractChanged
        )
    })
}

/// Requires the candidate descriptor to declare the recipe `strategy` invokes.
fn require_factory(
    candidate: &CandidateGraph,
    node: &PlannedNode,
    strategy: Option<ComponentTransitionStrategy>,
) -> crate::Result<()> {
    use ComponentTransitionStrategy::{HotUpdate, Replace, Transition};

    let (factory, reason) = match (candidate.component(node.component), strategy) {
        (Some(component), Some(Replace)) => (
            component.replacement_factory()?,
            RestartReason::ReplacementUnavailable,
        ),
        (Some(component), Some(Transition)) => (
            component.transition_handler()?,
            RestartReason::TransitionHandlerUnavailable,
        ),
        (Some(component), Some(HotUpdate)) => (
            component.hot_update_factory()?,
            RestartReason::HotUpdateUnavailable,
        ),
        (Some(component), _) => (
            component.effective_factory()?,
            RestartReason::FactoryUnavailable,
        ),
        (None, _) => (None, RestartReason::FactoryUnavailable),
    };

    if factory.is_some() {
        return Ok(());
    }

    restart(node, reason)
}

pub(super) fn restart<T>(node: &PlannedNode, reason: RestartReason) -> crate::Result<T> {
    Err(RestartRequired {
        component: node.component,
        required: Some(node.action),
        reason,
    }
    .into())
}
