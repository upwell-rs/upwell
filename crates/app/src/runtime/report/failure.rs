use std::fmt;

use upwell_config::ConfigReloadError;

use super::{RuntimeTransitionReport, TransitionOutcome, TransitionPhase};
use crate::RestartReason;

/// The redacted category of a rejected runtime transition attempt.
///
/// A category carries stable identities only: component IDs, binding paths, type names,
/// and [`RestartReason`]s. Error messages, config values, and panic payloads are never
/// included; the unredacted error is available from [`TransitionRejection::error`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(tag = "category", rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum TransitionFailure {
    /// Re-reading or re-merging the config sources failed.
    ConfigLoad,
    /// A changed binding failed to deserialize from the new config tree.
    ConfigBind {
        /// The binding's property path.
        path: String,
        /// The bound type's display name.
        type_name: &'static str,
    },
    /// A participant hook ended the attempt, by a rejecting verdict or an error. The
    /// rejecting phase names the hook kind; a verdict's reason code is in
    /// [`RuntimeTransitionReport::transition_hooks`].
    HookRejected {
        /// The display name of the component owning the hook, as the hook catalog
        /// records it (its type descriptor name, not its stable component ID).
        component: &'static str,
    },
    /// A participant hook panicked, ending the attempt. The payload is never recorded.
    HookPanicked {
        /// The display name of the component owning the hook, as the hook catalog
        /// records it (its type descriptor name, not its stable component ID).
        component: &'static str,
    },
    /// An application extension point (deserializer, condition callback, factory, or
    /// hook) panicked.
    Panicked,
    /// The condition catalog or its evaluation is invalid.
    ConditionInvalid,
    /// The candidate component graph failed validation or planning.
    GraphInvalid,
    /// The planned transition cannot be applied without a process restart.
    RestartRequired {
        /// The stable ID of the component that cannot transition in place.
        component: &'static str,
        /// Why the component requires a restart.
        reason: RestartReason,
    },
    /// The candidate was prepared against a generation that is no longer current.
    StaleBase,
    /// Constructing the candidate root failed.
    ConstructionFailed,
    /// Any other failure.
    Other,
}

impl TransitionFailure {
    /// Classifies an error raised during `phase` by its variant alone, never by its
    /// message, so nothing the error formats can reach the report.
    pub(crate) fn classify(phase: TransitionPhase, error: &crate::Error) -> Self {
        match error {
            crate::Error::ConfigReload(error) => Self::classify_config_reload(error),
            crate::Error::Config(_) => Self::ConfigLoad,
            crate::Error::CandidatePreparationPanicked => Self::Panicked,
            crate::Error::Condition(_) | crate::Error::ConditionEvaluationApplicationMismatch => {
                Self::ConditionInvalid
            }
            crate::Error::RestartRequired(required) => Self::RestartRequired {
                component: required.component,
                reason: required.reason,
            },
            crate::Error::StaleGraphCandidate(_) => Self::StaleBase,
            crate::Error::Di(error) => Self::classify_di(phase, error),
            crate::Error::TransitionHookRejected { component, .. } => {
                Self::HookRejected { component }
            }
            crate::Error::TransitionHookFailed {
                component, source, ..
            } => Self::classify_hook(component, source),
            crate::Error::DuplicateGenerationOverride { .. }
            | crate::Error::InvalidGenerationOverride { .. } => Self::ConstructionFailed,
            _ => Self::Other,
        }
    }

    fn classify_config_reload(error: &ConfigReloadError) -> Self {
        match error {
            ConfigReloadError::Load(_) => Self::ConfigLoad,
            ConfigReloadError::Bind {
                path, type_name, ..
            } => Self::ConfigBind {
                path: path.clone(),
                type_name,
            },
            ConfigReloadError::Hook { component, source } => Self::classify_hook(component, source),
            ConfigReloadError::Panicked => Self::Panicked,
        }
    }

    fn classify_hook(component: &'static str, error: &upwell_hooks::Error) -> Self {
        match error {
            upwell_hooks::Error::Panicked { .. } => Self::HookPanicked { component },
            _ => Self::HookRejected { component },
        }
    }

    fn classify_di(phase: TransitionPhase, error: &upwell_di::Error) -> Self {
        match error {
            upwell_di::Error::CandidateFactoryPanicked { .. }
            | upwell_di::Error::CandidateProviderPanicked { .. }
            | upwell_di::Error::CandidateFinalizationPanicked
            | upwell_di::Error::CandidateMetadataPanicked
            | upwell_di::Error::SnapshotPanicked { .. } => Self::Panicked,
            _ if phase == TransitionPhase::Construct => Self::ConstructionFailed,
            _ => Self::GraphInvalid,
        }
    }
}

impl fmt::Display for TransitionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigLoad => formatter.write_str("configuration could not be re-read"),
            Self::ConfigBind { path, type_name } => {
                write!(
                    formatter,
                    "binding '{path}' could not be re-bound as {type_name}"
                )
            }
            Self::HookRejected { component } => {
                write!(formatter, "a hook on '{component}' rejected the attempt")
            }
            Self::HookPanicked { component } => {
                write!(formatter, "a hook on '{component}' panicked")
            }
            Self::Panicked => formatter.write_str("an application extension point panicked"),
            Self::ConditionInvalid => formatter.write_str("condition evaluation is invalid"),
            Self::GraphInvalid => formatter.write_str("the candidate component graph is invalid"),
            Self::RestartRequired { component, reason } => {
                write!(
                    formatter,
                    "component '{component}' requires restart: {reason}"
                )
            }
            Self::StaleBase => formatter.write_str("the candidate's base generation is stale"),
            Self::ConstructionFailed => {
                formatter.write_str("the candidate root could not be constructed")
            }
            Self::Other => formatter.write_str("the transition failed"),
        }
    }
}

/// A rejected runtime transition attempt: its redacted report plus the error that
/// rejected it.
///
/// `Display` and `Debug` render only the redacted report. The original error is
/// reachable only through [`error`](Self::error), [`into_parts`](Self::into_parts), and
/// the [`source`](std::error::Error::source) chain; its message may contain
/// config-derived text, so do not forward it where values must stay redacted.
pub struct TransitionRejection {
    report: RuntimeTransitionReport,
    error: crate::Error,
}

impl TransitionRejection {
    pub(crate) fn new(report: RuntimeTransitionReport, error: crate::Error) -> Self {
        Self { report, error }
    }

    /// The redacted report of the rejected attempt.
    pub fn report(&self) -> &RuntimeTransitionReport {
        &self.report
    }

    /// The unredacted error that rejected the attempt.
    pub fn error(&self) -> &crate::Error {
        &self.error
    }

    /// Splits the rejection into its report and the error that rejected the attempt.
    pub fn into_parts(self) -> (RuntimeTransitionReport, crate::Error) {
        (self.report, self.error)
    }
}

impl fmt::Debug for TransitionRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransitionRejection")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for TransitionRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let attempt = self.report.attempt();

        match self.report.outcome() {
            TransitionOutcome::Rejected { phase, failure } => write!(
                formatter,
                "runtime transition attempt {attempt} was rejected during {phase}: {failure}"
            ),
            TransitionOutcome::Committed { .. } | TransitionOutcome::NoOp => {
                write!(
                    formatter,
                    "runtime transition attempt {attempt} was rejected"
                )
            }
        }
    }
}

impl std::error::Error for TransitionRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
