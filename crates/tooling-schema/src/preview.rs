//! What-if transition previews: the plan a proposed config change would produce, computed
//! by the target-local probe without constructing components or running trusted callbacks.
//!
//! The preview mirrors the runtime transition report's shape and redaction: stable
//! identities, planner categories and reason paths, changed binding paths and type names,
//! never config values.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Environment variable carrying a JSON [`TransitionPreviewRequest`] for one probe.
pub const TOOLING_PROBE_TRANSITION_PREVIEW_ENV: &str = "UPWELL_TOOLING_PROBE_TRANSITION_PREVIEW";

/// One proposed config assignment: a dotted property path and a scalar value literal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfigAssignment {
    /// Dot-separated property path.
    pub path: String,
    /// Scalar literal: `true`/`false`, a number, a `"quoted string"`, or bare text.
    pub value: String,
}

/// The proposed change a probe previews against its prepared application.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransitionPreviewRequest {
    /// Assignments merged above every config source, in order; later ones win.
    #[serde(default)]
    pub assignments: Vec<ConfigAssignment>,
}

/// Failure while decoding a [`TransitionPreviewRequest`] from its environment value.
#[derive(Debug, Error)]
#[error("invalid transition preview request")]
pub struct TransitionPreviewRequestError(#[source] serde_json::Error);

impl TransitionPreviewRequest {
    /// Encodes this request for [`TOOLING_PROBE_TRANSITION_PREVIEW_ENV`].
    pub fn to_env_value(&self) -> String {
        serde_json::to_string(self).expect("string-only requests always serialize")
    }

    /// Decodes a request from [`TOOLING_PROBE_TRANSITION_PREVIEW_ENV`].
    pub fn from_env_value(value: &str) -> Result<Self, TransitionPreviewRequestError> {
        serde_json::from_str(value).map_err(TransitionPreviewRequestError)
    }
}

/// The redacted plan a proposed config change would produce.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransitionPreview {
    /// The assigned property paths, in request order. Values are never echoed.
    pub assignments: Vec<String>,
    /// Whether the change would commit, change nothing, or be rejected.
    #[serde(flatten)]
    pub outcome: PreviewOutcome,
    /// Whether tooling decided every condition of the proposed graph.
    pub evaluation: PreviewEvaluation,
    /// The bindings whose value the change alters: paths and type names.
    pub changed: Vec<PreviewChangedBinding>,
    /// Conditional components whose condition outcome the change alters.
    pub eligibility: Vec<EligibilityChange>,
    /// Every affected component in the planner's stable component order.
    pub components: Vec<PreviewComponent>,
    /// The singletons a commit would displace, in retirement order.
    pub displaced: Vec<String>,
}

/// The planned outcome of a previewed transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewOutcome {
    /// The change would publish a new runtime generation.
    Planned,
    /// The change would leave the runtime generation unchanged.
    NoOp,
    /// The change would be rejected before construction and keep the active generation.
    Rejected {
        /// The rejecting phase.
        phase: PreviewPhase,
        /// The redacted failure category, as in runtime transition reports.
        failure: PreviewFailure,
    },
}

/// Whether tooling decided every condition of the proposed graph.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewEvaluation {
    /// Every condition was decided.
    Complete,
    /// A trusted callback left some conditions undecided; their components are excluded.
    Partial,
}

/// One binding whose value the previewed change alters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewChangedBinding {
    /// The binding's property path.
    pub path: String,
    /// The bound type's display name.
    pub type_name: String,
}

/// One conditional component whose condition outcome the change alters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EligibilityChange {
    /// The component's stable ID.
    pub component: String,
    /// The outcome before the change.
    pub before: PreviewConditionOutcome,
    /// The outcome after the change.
    pub after: PreviewConditionOutcome,
}

/// One affected component: its stable identity, the planner's action, the strategy the
/// runtime would choose, and why.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewComponent {
    /// The component's stable ID.
    pub component: String,
    /// The selected factory's stable ID, when the component has one.
    #[serde(default)]
    pub factory: Option<String>,
    /// The component's runtime role.
    pub role: PreviewRole,
    /// The minimum structural action the planner requires.
    pub action: PreviewAction,
    /// The strategy the runtime would apply. Absent for a removed component, and for a
    /// scoped or transient component, which has no root instance and is rebuilt from the
    /// published scope plan on its next opening.
    #[serde(default)]
    pub strategy: Option<PreviewStrategy>,
    /// The canonical reason paths from each direct change to this component.
    pub reasons: Vec<PreviewReason>,
}

/// One reason path: its category and the dependency demands it crosses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewReason {
    /// The planner's reason category.
    pub kind: PreviewReasonKind,
    /// The dependency demands from the direct change to the affected component.
    pub path: Vec<PreviewDemand>,
}

/// One dependency demand on a reason path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewDemand {
    /// The consuming component's stable ID.
    pub consumer: String,
    /// The demand's position in the consumer's selected factory.
    pub ordinal: usize,
    /// The requested type, when the demand is still part of a known graph.
    #[serde(default)]
    pub requested: Option<String>,
}

/// A phase that can reject a previewed transition before construction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewPhase {
    /// Staging the proposed config and its changed bindings.
    Stage,
    /// Evaluating conditions against the proposed config.
    Evaluate,
    /// Preparing the candidate graph and resolving the transition plan.
    Plan,
}

/// The redacted category of a rejected preview, as in runtime transition reports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "category", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewFailure {
    /// Re-reading or re-merging the config sources failed.
    ConfigLoad,
    /// A changed binding failed to deserialize from the proposed tree.
    ConfigBind {
        /// The binding's property path.
        path: String,
        /// The bound type's display name.
        type_name: String,
    },
    /// An application extension point panicked.
    Panicked,
    /// The condition catalog or its evaluation is invalid.
    ConditionInvalid,
    /// The proposed component graph failed validation or planning.
    GraphInvalid,
    /// The planned transition cannot be applied without a process restart.
    RestartRequired {
        /// The stable ID of the component that cannot transition in place.
        component: String,
        /// The runtime's restart reason, in its snake-case form.
        reason: String,
    },
    /// Any other failure.
    Other,
}

/// The tooling outcome of one component condition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewConditionOutcome {
    /// The condition holds, so the component is eligible.
    Holds,
    /// The condition fails, so the component is excluded.
    Fails,
    /// A trusted callback left the condition undecided.
    NotEvaluated,
}

/// The runtime role of an affected component.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewRole {
    /// A stored root instance.
    Singleton,
    /// A recipe used by future openings of one declared scope.
    FutureScope {
        /// The scope's stable namespaced ID.
        scope: String,
    },
    /// A recipe executed for each resolution.
    Transient,
}

/// The minimum structural action the planner requires for one component.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewAction {
    /// Keep the instance unchanged.
    Retain,
    /// Republish changed live dependencies.
    RebindLive,
    /// Build a new instance in place of the active one.
    Replace,
    /// Build a component the active graph lacks.
    Add,
    /// Drop a component the proposed graph lacks.
    Remove,
}

/// The component-declared strategy the runtime would apply.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewStrategy {
    /// Keep the instance, rebinding live dependencies in place.
    Retain,
    /// Run the ordinary factory again.
    Reconstruct,
    /// Run the static replacement factory over the previous instance.
    Replace,
    /// Ask the transition handler for a one-shot factory.
    Transition,
    /// Update the retained instance in place at commit.
    HotUpdate,
}

/// Why the planner requires an action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreviewReasonKind {
    /// The component joins the graph.
    Added,
    /// The component leaves the graph.
    Removed,
    /// Its selected factory changed.
    FactoryChanged,
    /// Its descriptor identity changed.
    IdentityChanged,
    /// Its declared dependency contract changed.
    DependencyContractChanged,
    /// A fixed dependency now targets another provider.
    DependencyTargetChanged,
    /// A live dependency now targets another provider.
    LiveTargetChanged,
    /// A fixed dependency is itself affected.
    FixedDependent,
    /// A live dependency is itself affected.
    LiveDependent,
    /// A changed binding, config-reload hook, or hook manager invalidates it.
    RuntimeInvalidated,
}

impl PreviewEvaluation {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
        }
    }
}

impl PreviewPhase {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Evaluate => "evaluate",
            Self::Plan => "plan",
        }
    }
}

impl PreviewConditionOutcome {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Holds => "holds",
            Self::Fails => "fails",
            Self::NotEvaluated => "not_evaluated",
        }
    }
}

impl PreviewAction {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::RebindLive => "rebind_live",
            Self::Replace => "replace",
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }
}

impl PreviewStrategy {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::Reconstruct => "reconstruct",
            Self::Replace => "replace",
            Self::Transition => "transition",
            Self::HotUpdate => "hot_update",
        }
    }
}

impl PreviewReasonKind {
    /// The stable snake-case name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::FactoryChanged => "factory_changed",
            Self::IdentityChanged => "identity_changed",
            Self::DependencyContractChanged => "dependency_contract_changed",
            Self::DependencyTargetChanged => "dependency_target_changed",
            Self::LiveTargetChanged => "live_target_changed",
            Self::FixedDependent => "fixed_dependent",
            Self::LiveDependent => "live_dependent",
            Self::RuntimeInvalidated => "runtime_invalidated",
        }
    }
}

#[cfg(test)]
mod tests;
