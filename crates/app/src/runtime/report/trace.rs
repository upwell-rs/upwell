use tracing::{debug, info, warn};
use upwell_core::RuntimeGenerationId;

use super::{RuntimeTransitionReport, TransitionAttemptId, TransitionOutcome, TransitionPhase};

/// The log target every runtime transition event uses.
pub(crate) const TRANSITION_TARGET: &str = "upwell::transition";

/// Emits one structured event per affected component, then one terminal event, with the
/// report's stable redacted fields.
pub(super) fn report(report: &RuntimeTransitionReport) {
    let attempt = report.attempt().get();
    let base_generation = report.base_generation().get();
    let elapsed_us = u64::try_from(report.elapsed().as_micros()).unwrap_or(u64::MAX);

    for component in report.components() {
        let reasons = component
            .reasons
            .iter()
            .map(|reason| reason.kind)
            .collect::<Vec<_>>();

        debug!(
            target: TRANSITION_TARGET,
            attempt,
            component = component.component,
            factory = component.factory,
            action = ?component.action,
            reasons = ?reasons,
            "runtime transition affects component"
        );
    }

    match report.outcome() {
        TransitionOutcome::Committed { generation } => info!(
            target: TRANSITION_TARGET,
            attempt,
            base_generation,
            generation = generation.get(),
            config_generation = report.config_generation(),
            changed = report.changed().len(),
            affected = report.components().len(),
            hooks = report.hooks().len(),
            elapsed_us,
            outcome = "committed",
            "runtime transition committed"
        ),
        TransitionOutcome::NoOp => debug!(
            target: TRANSITION_TARGET,
            attempt,
            base_generation,
            elapsed_us,
            outcome = "no_op",
            "runtime transition was a no-op"
        ),
        TransitionOutcome::Rejected { phase, failure } => warn!(
            target: TRANSITION_TARGET,
            attempt,
            base_generation,
            changed = report.changed().len(),
            affected = report.components().len(),
            elapsed_us,
            outcome = "rejected",
            phase = phase.as_str(),
            failure = %failure,
            "runtime transition rejected"
        ),
    }
}

/// Emits the terminal event of an attempt whose future was dropped before it finished.
pub(super) fn cancelled(
    attempt: TransitionAttemptId,
    base_generation: RuntimeGenerationId,
    phase: Option<TransitionPhase>,
) {
    warn!(
        target: TRANSITION_TARGET,
        attempt = attempt.get(),
        base_generation = base_generation.get(),
        phase = phase.map(TransitionPhase::as_str),
        outcome = "cancelled",
        "runtime transition cancelled"
    );
}
