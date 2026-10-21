//! Canonical, redacted reports of runtime transition attempts.
//!
//! Every attempt that acquires the transition writer produces exactly one
//! [`RuntimeTransitionReport`], whether it commits, is a no-op, or is rejected. A
//! rejected attempt returns its report inside
//! [`Error::TransitionRejected`](crate::Error::TransitionRejected).
//!
//! # Redaction
//!
//! Reports carry stable identities and categories only: attempt and generation IDs, the
//! reload cause, component and factory IDs, planner actions and reason paths, changed
//! binding paths and type names, hook outcomes, and phase timings. They never carry current or proposed
//! config values, deserializer output, or panic payloads, in `Debug`, `Display`,
//! serialized output, or the `upwell::transition` tracing events. A
//! [`TransitionRejection`] renders only its report in `Debug` and `Display` too; the
//! unredacted error that rejected an attempt is reachable only through
//! [`TransitionRejection::error`], [`TransitionRejection::into_parts`], and the standard
//! error source chain.

use std::fmt;
use std::time::Duration;

use upwell_config::{ChangedBinding, ComponentHookReport, ReloadCause};
use upwell_core::RuntimeGenerationId;
use upwell_di::{EffectiveGraph, EffectiveNodeRole, NodeAction, TransitionPlan, TransitionReason};

pub use super::generation::TransitionAttemptId;

mod failure;
mod hooks;
mod recorder;
#[cfg(feature = "serde")]
mod serialize;
mod trace;

pub use failure::{TransitionFailure, TransitionRejection};
pub use hooks::{TransitionHookOutcome, TransitionHookReport};
pub(crate) use recorder::{TransitionCompletion, TransitionRecorder};
pub(crate) use trace::TRANSITION_TARGET;

/// One phase of a runtime transition attempt, in execution order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum TransitionPhase {
    /// Re-reading configuration and staging changed bindings.
    Stage,
    /// Re-evaluating component conditions against the staged values.
    Evaluate,
    /// Preparing the candidate graph and resolving the transition plan.
    Plan,
    /// Running transition-proposal hooks against the active generation, before any
    /// candidate construction.
    Proposal,
    /// Constructing the candidate root and prepared generation.
    Construct,
    /// Running the affected config-reload hooks against the candidate.
    Hooks,
    /// Running transition-decision hooks against the constructed candidate,
    /// immediately before commit.
    Decision,
    /// Publishing the candidate and committing the staged config.
    Commit,
}

impl TransitionPhase {
    /// The phase's stable snake-case name, as used in tracing and serialized reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Evaluate => "evaluate",
            Self::Plan => "plan",
            Self::Proposal => "proposal",
            Self::Construct => "construct",
            Self::Hooks => "hooks",
            Self::Decision => "decision",
            Self::Commit => "commit",
        }
    }
}

impl fmt::Display for TransitionPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The elapsed wall-clock time of one phase an attempt entered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct PhaseTiming {
    /// The phase that ran.
    pub phase: TransitionPhase,
    /// How long the phase ran, including a phase that ended in rejection.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "elapsed_us", serialize_with = "serialize::duration_micros")
    )]
    pub elapsed: Duration,
}

/// The terminal outcome of one runtime transition attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(tag = "outcome", rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum TransitionOutcome {
    /// A new runtime generation was published.
    Committed {
        /// The newly published runtime generation.
        #[cfg_attr(feature = "serde", serde(serialize_with = "serialize::generation"))]
        generation: RuntimeGenerationId,
    },
    /// Nothing changed, so no generation was published.
    NoOp,
    /// The attempt was rejected and the previous generation stays active.
    Rejected {
        /// The phase that rejected the attempt.
        phase: TransitionPhase,
        /// The redacted failure category.
        failure: TransitionFailure,
    },
}

/// One affected component in a transition plan: its stable identity, the action the
/// planner requires, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct ComponentTransitionReport {
    /// The component's stable ID.
    pub component: &'static str,
    /// The selected factory's stable ID: the candidate factory, or for a removed
    /// component the active one. `None` for a component without a factory.
    pub factory: Option<&'static str>,
    /// The component's runtime role.
    pub role: EffectiveNodeRole,
    /// The minimum structural action the planner requires.
    pub action: NodeAction,
    /// The canonical reason paths from each direct change to this component.
    pub reasons: Box<[TransitionReason]>,
}

impl ComponentTransitionReport {
    /// Projects every planned node in deterministic plan order. Factories are looked up
    /// in the candidate graph first and in the active graph for removed components.
    pub(crate) fn project(
        plan: &TransitionPlan,
        active: &EffectiveGraph,
        candidate: &EffectiveGraph,
    ) -> Box<[Self]> {
        plan.nodes
            .iter()
            .map(|node| {
                let factory = candidate
                    .node(node.component)
                    .or_else(|| active.node(node.component))
                    .and_then(|effective| effective.factory)
                    .map(|factory| factory.id);

                Self {
                    component: node.component,
                    factory,
                    role: node.role,
                    action: node.action,
                    reasons: node.reasons.clone(),
                }
            })
            .collect()
    }
}

/// The canonical, redacted report of one runtime transition attempt.
///
/// Produced for every attempt that acquires the transition writer: committed, no-op, and
/// rejected. Everything except [`phases`](Self::phases) is deterministic for identical
/// inputs; affected components follow the planner's stable component order.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RuntimeTransitionReport {
    attempt: TransitionAttemptId,
    cause: ReloadCause,
    #[cfg_attr(feature = "serde", serde(serialize_with = "serialize::generation"))]
    base_generation: RuntimeGenerationId,
    #[cfg_attr(feature = "serde", serde(flatten))]
    outcome: TransitionOutcome,
    config_generation: u64,
    #[cfg_attr(feature = "serde", serde(serialize_with = "serialize::changed"))]
    changed: Box<[ChangedBinding]>,
    components: Box<[ComponentTransitionReport]>,
    displaced: Box<[&'static str]>,
    #[cfg_attr(feature = "serde", serde(serialize_with = "serialize::hooks"))]
    hooks: Box<[ComponentHookReport]>,
    transition_hooks: Box<[TransitionHookReport]>,
    phases: Box<[PhaseTiming]>,
}

impl RuntimeTransitionReport {
    /// The attempt's identity. No-op and rejected attempts consume one too.
    pub fn attempt(&self) -> TransitionAttemptId {
        self.attempt
    }

    /// What requested the attempt: a programmatic request, `SIGHUP`, or a watched file
    /// change.
    pub fn cause(&self) -> ReloadCause {
        self.cause
    }

    /// The committed generation the attempt was prepared against.
    pub fn base_generation(&self) -> RuntimeGenerationId {
        self.base_generation
    }

    /// The attempt's terminal outcome.
    pub fn outcome(&self) -> &TransitionOutcome {
        &self.outcome
    }

    /// Whether the attempt published a new runtime generation.
    pub fn is_committed(&self) -> bool {
        matches!(self.outcome, TransitionOutcome::Committed { .. })
    }

    /// The runtime generation current after the attempt: the published generation when
    /// it committed, otherwise the unchanged base.
    pub fn runtime_generation(&self) -> RuntimeGenerationId {
        match self.outcome {
            TransitionOutcome::Committed { generation } => generation,
            TransitionOutcome::NoOp | TransitionOutcome::Rejected { .. } => self.base_generation,
        }
    }

    /// The config generation after the attempt. It advances only when the attempt
    /// committed.
    pub fn config_generation(&self) -> u64 {
        self.config_generation
    }

    /// The bindings whose source changed: paths and type names, never values. Empty for
    /// a no-op and for an attempt rejected before staging finished.
    pub fn changed(&self) -> &[ChangedBinding] {
        &self.changed
    }

    /// The components the transition plan affects, in stable component order. Empty
    /// when the attempt ended before planning finished.
    pub fn components(&self) -> &[ComponentTransitionReport] {
        &self.components
    }

    /// The affected component with the given stable ID.
    pub fn component(&self, id: &str) -> Option<&ComponentTransitionReport> {
        self.components
            .iter()
            .find(|component| component.component == id)
    }

    /// The stable IDs of the singletons the committed generation displaced (constructed
    /// again or removed), in retirement order: every consumer precedes the components it holds a
    /// fixed dependency on. Components linked only through live dependencies may retire
    /// in either order. Each receives [`ComponentRetired`](crate::ComponentRetired) once. Empty unless
    /// the attempt committed.
    pub fn displaced(&self) -> &[&'static str] {
        &self.displaced
    }

    /// The config-reload hooks that ran and accepted. Empty when no hook ran or a hook
    /// rejected the attempt.
    pub fn hooks(&self) -> &[ComponentHookReport] {
        &self.hooks
    }

    /// The transition-proposal and transition-decision hooks that ran, in firing order,
    /// including a participant that rejected the attempt.
    pub fn transition_hooks(&self) -> &[TransitionHookReport] {
        &self.transition_hooks
    }

    /// The timing of every phase the attempt entered, in execution order.
    pub fn phases(&self) -> &[PhaseTiming] {
        &self.phases
    }

    /// The total elapsed time across every phase the attempt entered.
    pub fn elapsed(&self) -> Duration {
        self.phases.iter().map(|timing| timing.elapsed).sum()
    }
}

#[cfg(test)]
mod tests;
