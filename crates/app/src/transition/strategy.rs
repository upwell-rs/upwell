//! Component-owned transition strategies and the restart reasons that reject them.

use std::fmt;

use upwell_di::{EffectiveNodeRole, NodeAction, PlannedNode};

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
            Self::LiveRebindUnsupported => {
                "retained live dependency rebinding is not integrated yet"
            }
            Self::RemovalUnsupported => "component removal is not integrated yet",
            Self::NonSingleton => "runtime transitions are singleton-only",
            Self::FactoryUnavailable => "the candidate component has no ordinary factory",
            Self::ReplacementUnavailable => {
                "the candidate component declares no replacement factory"
            }
            Self::PreviousUnavailable => "the component has no active instance to replace",
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
pub(super) fn validate_decision(
    candidate: &CandidateGraph,
    node: &PlannedNode,
    strategy: Option<ComponentTransitionStrategy>,
) -> crate::Result<()> {
    use ComponentTransitionStrategy::{Reconstruct, Replace, Retain};

    if node.role != EffectiveNodeRole::Singleton {
        return restart(node, RestartReason::NonSingleton);
    }

    match (node.action, strategy) {
        (NodeAction::Remove, _) => restart(node, RestartReason::RemovalUnsupported),
        (NodeAction::RebindLive, Some(Retain)) => {
            restart(node, RestartReason::LiveRebindUnsupported)
        }
        (NodeAction::Replace, Some(Retain)) => restart(node, RestartReason::StaleRetention),
        (NodeAction::Retain, Some(Retain)) => Ok(()),
        (NodeAction::Add | NodeAction::Replace | NodeAction::RebindLive, Some(Reconstruct)) => {
            require_factory(candidate, node, Reconstruct)
        }
        (NodeAction::Replace | NodeAction::RebindLive, Some(Replace)) => {
            require_factory(candidate, node, Replace)
        }
        (NodeAction::Add, Some(Replace)) => restart(node, RestartReason::PreviousUnavailable),
        _ => restart(node, RestartReason::MissingDecision),
    }
}

/// Requires the candidate descriptor to declare the factory `strategy` invokes.
fn require_factory(
    candidate: &CandidateGraph,
    node: &PlannedNode,
    strategy: ComponentTransitionStrategy,
) -> crate::Result<()> {
    let (factory, reason) = match (candidate.component(node.component), strategy) {
        (Some(component), ComponentTransitionStrategy::Replace) => (
            component.replacement_factory()?,
            RestartReason::ReplacementUnavailable,
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
