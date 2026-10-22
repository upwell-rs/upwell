//! What-if transition previews over a prepared application.
//!
//! A preview runs the transactional reload's stage, evaluate and plan phases against a
//! config overlay and stops before construction. It shares the reload's snapshot and
//! planning code, so a preview and a real reload of the same change plan identically,
//! except that conditions are evaluated as tooling evaluates them: structured predicates
//! and pure callbacks only, never trusted callbacks.

use std::panic::AssertUnwindSafe;

use upwell_config::{ConfigOverlay, StagedReload};
use upwell_core::RuntimeGenerationId;
use upwell_di::{
    ConditionOutcome, EffectiveGraph, EffectiveNodeRole, NodeAction, ReasonKind, TransitionReason,
};
use upwell_tooling_schema::{
    EligibilityChange, PreviewAction, PreviewChangedBinding, PreviewConditionOutcome,
    PreviewDemand, PreviewEvaluation, PreviewFailure, PreviewOutcome, PreviewPhase, PreviewReason,
    PreviewReasonKind, PreviewRole, PreviewStrategy, TransitionPreview,
};

use crate::registry::AppConditionEvaluation;
use crate::runtime::{
    ComponentTransitionReport, TransitionFailure, TransitionPhase, resolve_staged_transition,
    staged_condition_snapshot,
};
use crate::transition::{CandidateGraph, ComponentTransitionStrategy, ResolvedTransitionPlan};
use crate::{PreparedApp, ProtocolDefinition};

impl<D: ProtocolDefinition> PreparedApp<D> {
    /// Plans the transition `overlay` would produce against this prepared application.
    ///
    /// The overlay merges above the re-read config sources, and changed bindings are
    /// deserialized as a reload would. Conditions are then evaluated without running
    /// trusted callbacks, and the candidate graph and transition plan are resolved. No
    /// component factory, hook, or trusted callback runs, nothing is published, and the
    /// preview carries no config values. A panicking extension point is reported as a
    /// rejection.
    pub fn transition_preview(&self, overlay: &ConfigOverlay) -> TransitionPreview {
        let mut preview = TransitionPreview {
            assignments: overlay.paths().to_vec(),
            outcome: PreviewOutcome::NoOp,
            evaluation: evaluation_kind(self.condition_state().evaluation()),
            changed: Vec::new(),
            eligibility: Vec::new(),
            components: Vec::new(),
            displaced: Vec::new(),
        };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.plan_preview(overlay, &mut preview)
        }))
        .unwrap_or(Err((
            TransitionPhase::Plan,
            crate::Error::CandidatePreparationPanicked,
        )));

        preview.outcome = match result {
            Ok(outcome) => outcome,
            Err((phase, error)) => PreviewOutcome::Rejected {
                phase: preview_phase(phase),
                failure: preview_failure(TransitionFailure::classify(phase, &error)),
            },
        };

        preview
    }

    fn plan_preview(
        &self,
        overlay: &ConfigOverlay,
        preview: &mut TransitionPreview,
    ) -> Result<PreviewOutcome, (TransitionPhase, crate::Error)> {
        let state = self.condition_state();
        let base = state.evaluation();
        let staged = self
            .config_reloader()
            .stage_with(overlay)
            .map_err(|error| (TransitionPhase::Stage, error.into()))?;

        preview.changed = staged
            .changed()
            .iter()
            .map(|binding| PreviewChangedBinding {
                path: binding.path.clone(),
                type_name: binding.type_name.to_string(),
            })
            .collect();

        if staged.is_empty() && !base.reads_raw_config() {
            return Ok(PreviewOutcome::NoOp);
        }

        let evaluation = staged_condition_snapshot(state.catalog(), base, &staged)
            .and_then(|snapshot| base.evaluate_for_tooling(&snapshot))
            .map_err(|error| (TransitionPhase::Evaluate, error))?;

        preview.evaluation = evaluation_kind(&evaluation);
        preview.eligibility = self.eligibility_changes(base, &evaluation);

        if staged.is_empty() && evaluation.evaluation().same_eligibility(base.evaluation()) {
            return Ok(PreviewOutcome::NoOp);
        }

        let (candidate, resolved) = self
            .plan_candidate(&evaluation, &staged)
            .map_err(|error| (TransitionPhase::Plan, error))?;

        preview.components = preview_components(&resolved, self.effective_graph(), &candidate);
        preview.displaced = resolved
            .displaced()
            .iter()
            .map(ToString::to_string)
            .collect();

        Ok(PreviewOutcome::Planned)
    }

    fn plan_candidate(
        &self,
        evaluation: &AppConditionEvaluation,
        staged: &StagedReload,
    ) -> crate::Result<(CandidateGraph, ResolvedTransitionPlan)> {
        let candidate = CandidateGraph::prepare_evaluation(
            RuntimeGenerationId::INITIAL,
            self.condition_state().catalog(),
            evaluation,
            self.scope_topology(),
        )?;
        let resolved = resolve_staged_transition(&candidate, self.effective_graph(), staged)?;

        Ok((candidate, resolved))
    }

    /// The conditional catalog components whose outcome differs between `before` and
    /// `after`, in catalog order.
    fn eligibility_changes(
        &self,
        before: &AppConditionEvaluation,
        after: &AppConditionEvaluation,
    ) -> Vec<EligibilityChange> {
        self.condition_state()
            .catalog()
            .components
            .iter()
            .filter(|component| component.condition.is_some())
            .filter_map(|component| {
                let outcome = |evaluation: &AppConditionEvaluation| {
                    evaluation
                        .evaluation()
                        .component_outcome(component.id)
                        .unwrap_or(ConditionOutcome::NotEvaluated)
                };
                let (before, after) = (outcome(before), outcome(after));

                (before != after).then(|| EligibilityChange {
                    component: component.id.to_string(),
                    before: condition_outcome(before),
                    after: condition_outcome(after),
                })
            })
            .collect()
    }
}

fn preview_components(
    resolved: &ResolvedTransitionPlan,
    active: &EffectiveGraph,
    candidate: &CandidateGraph,
) -> Vec<upwell_tooling_schema::PreviewComponent> {
    let strategy = |component: &str| {
        resolved
            .decisions()
            .iter()
            .find(|decision| decision.component == component)
            .map(|decision| preview_strategy(decision.strategy))
    };

    ComponentTransitionReport::project(resolved.structural(), active, candidate.graph())
        .iter()
        .map(|report| upwell_tooling_schema::PreviewComponent {
            component: report.component.to_string(),
            factory: report.factory.map(str::to_string),
            role: preview_role(report.role),
            action: preview_action(report.action),
            strategy: strategy(report.component),
            reasons: report
                .reasons
                .iter()
                .map(|reason| preview_reason(reason, active, candidate.graph()))
                .collect(),
        })
        .collect()
}

/// A reason path whose demands name their requested type, looked up in the candidate
/// graph first and in the active graph for removed consumers.
fn preview_reason(
    reason: &TransitionReason,
    active: &EffectiveGraph,
    candidate: &EffectiveGraph,
) -> PreviewReason {
    let requested = |consumer: &str, ordinal: usize| {
        candidate
            .node(consumer)
            .or_else(|| active.node(consumer))
            .and_then(|node| node.dependencies.get(ordinal))
            .map(|demand| demand.requested_type.to_string())
    };

    PreviewReason {
        kind: preview_reason_kind(reason.kind),
        path: reason
            .path
            .iter()
            .map(|demand| PreviewDemand {
                consumer: demand.consumer.to_string(),
                ordinal: demand.ordinal,
                requested: requested(demand.consumer, demand.ordinal),
            })
            .collect(),
    }
}

fn evaluation_kind(evaluation: &AppConditionEvaluation) -> PreviewEvaluation {
    if evaluation.evaluation().is_complete() {
        PreviewEvaluation::Complete
    } else {
        PreviewEvaluation::Partial
    }
}

fn preview_phase(phase: TransitionPhase) -> PreviewPhase {
    match phase {
        TransitionPhase::Stage => PreviewPhase::Stage,
        TransitionPhase::Evaluate => PreviewPhase::Evaluate,
        _ => PreviewPhase::Plan,
    }
}

fn preview_failure(failure: TransitionFailure) -> PreviewFailure {
    match failure {
        TransitionFailure::ConfigLoad => PreviewFailure::ConfigLoad,
        TransitionFailure::ConfigBind { path, type_name } => PreviewFailure::ConfigBind {
            path,
            type_name: type_name.to_string(),
        },
        TransitionFailure::Panicked => PreviewFailure::Panicked,
        TransitionFailure::ConditionInvalid => PreviewFailure::ConditionInvalid,
        TransitionFailure::GraphInvalid => PreviewFailure::GraphInvalid,
        TransitionFailure::RestartRequired { component, reason } => {
            PreviewFailure::RestartRequired {
                component: component.to_string(),
                reason: reason.as_str().to_string(),
            }
        }
        _ => PreviewFailure::Other,
    }
}

fn condition_outcome(outcome: ConditionOutcome) -> PreviewConditionOutcome {
    match outcome {
        ConditionOutcome::Holds => PreviewConditionOutcome::Holds,
        ConditionOutcome::Fails => PreviewConditionOutcome::Fails,
        ConditionOutcome::NotEvaluated => PreviewConditionOutcome::NotEvaluated,
    }
}

fn preview_role(role: EffectiveNodeRole) -> PreviewRole {
    match role {
        EffectiveNodeRole::Singleton => PreviewRole::Singleton,
        EffectiveNodeRole::FutureScope(scope) => PreviewRole::FutureScope {
            scope: scope.as_str().to_string(),
        },
        EffectiveNodeRole::Transient => PreviewRole::Transient,
    }
}

fn preview_action(action: NodeAction) -> PreviewAction {
    match action {
        NodeAction::Retain => PreviewAction::Retain,
        NodeAction::RebindLive => PreviewAction::RebindLive,
        NodeAction::Replace => PreviewAction::Replace,
        NodeAction::Add => PreviewAction::Add,
        NodeAction::Remove => PreviewAction::Remove,
    }
}

fn preview_strategy(strategy: ComponentTransitionStrategy) -> PreviewStrategy {
    match strategy {
        ComponentTransitionStrategy::Retain => PreviewStrategy::Retain,
        ComponentTransitionStrategy::Reconstruct => PreviewStrategy::Reconstruct,
        ComponentTransitionStrategy::Replace => PreviewStrategy::Replace,
        ComponentTransitionStrategy::Transition => PreviewStrategy::Transition,
        ComponentTransitionStrategy::HotUpdate => PreviewStrategy::HotUpdate,
    }
}

fn preview_reason_kind(kind: ReasonKind) -> PreviewReasonKind {
    match kind {
        ReasonKind::Added => PreviewReasonKind::Added,
        ReasonKind::Removed => PreviewReasonKind::Removed,
        ReasonKind::FactoryChanged => PreviewReasonKind::FactoryChanged,
        ReasonKind::IdentityChanged => PreviewReasonKind::IdentityChanged,
        ReasonKind::DependencyContractChanged => PreviewReasonKind::DependencyContractChanged,
        ReasonKind::DependencyTargetChanged => PreviewReasonKind::DependencyTargetChanged,
        ReasonKind::LiveTargetChanged => PreviewReasonKind::LiveTargetChanged,
        ReasonKind::FixedDependent => PreviewReasonKind::FixedDependent,
        ReasonKind::LiveDependent => PreviewReasonKind::LiveDependent,
        ReasonKind::RuntimeInvalidated => PreviewReasonKind::RuntimeInvalidated,
    }
}

#[cfg(test)]
mod tests;
