//! The phases of one reload attempt that need more than a call into [`stages`](super::stages).

use upwell_config::StagedReload;
use upwell_di::context::InvocationContext;
use upwell_hooks::HookManager;

use super::super::generation::RuntimeTransition;
use super::super::report::{ComponentTransitionReport, TransitionPhase, TransitionRecorder};
use super::super::transition_hooks::{
    AffectedComponents, HookOwners, PendingTransition, TransitionDecision, fire_gate,
};
use super::super::{AppRuntime, RuntimeView};
use super::stages::{evaluate_conditions, plan_transition};
use crate::registry::AppConditionEvaluation;
use crate::transition::{CandidateGraph, ResolvedTransitionPlan};

/// A planned candidate and the plan that resolved it.
pub(super) struct PlannedReload {
    pub(super) candidate: CandidateGraph,
    pub(super) resolved_plan: ResolvedTransitionPlan,
    pub(super) hooks: HookPlan,
}

/// What the proposal and decision hook phases need to run against a planned candidate.
pub(super) struct HookPlan {
    pub(super) affected: AffectedComponents,
    pub(super) active_owners: HookOwners,
    pub(super) candidate_owners: HookOwners,
    pub(super) pending: PendingTransition,
}

impl AppRuntime {
    /// Stages the config source and evaluates conditions against it, or returns `None` when the
    /// attempt is a true no-op: nothing changed and eligibility is unchanged.
    pub(super) fn stage_and_evaluate(
        &self,
        base: &RuntimeView,
        recorder: &mut TransitionRecorder,
    ) -> crate::Result<Option<(StagedReload, AppConditionEvaluation)>> {
        recorder.enter(TransitionPhase::Stage);

        let staged = self.reloader.stage()?;

        recorder.record_changed(staged.changed());

        if staged.is_empty() && !base.condition().evaluation().reads_raw_config() {
            return Ok(None);
        }

        recorder.enter(TransitionPhase::Evaluate);

        let evaluation = evaluate_conditions(base, &staged)?;

        if staged.is_empty()
            && evaluation
                .evaluation()
                .same_eligibility(base.condition().evaluation().evaluation())
        {
            return Ok(None);
        }

        Ok(Some((staged, evaluation)))
    }
}

/// Plans the candidate graph for `staged` and records the component projection and the
/// displaced singletons.
pub(super) fn plan_candidate(
    transition: &RuntimeTransition,
    base: &RuntimeView,
    staged: &StagedReload,
    evaluation: &AppConditionEvaluation,
    recorder: &mut TransitionRecorder,
) -> crate::Result<PlannedReload> {
    recorder.enter(TransitionPhase::Plan);

    let (candidate, resolved_plan) = plan_transition(base, staged, evaluation)?;
    let components = ComponentTransitionReport::project(
        resolved_plan.structural(),
        base.effective_graph(),
        candidate.graph(),
    );
    let affected = AffectedComponents::new(
        &components,
        base.resolved_components(),
        candidate.components(),
    );
    let active_owners = HookOwners::new(base.resolved_components());
    let candidate_owners = HookOwners::new(candidate.components());
    let pending = PendingTransition::proposed(
        transition.attempt(),
        base.id(),
        staged.changed(),
        &components,
    );

    recorder.record_components(components);
    recorder.record_displaced(resolved_plan.displaced());

    Ok(PlannedReload {
        candidate,
        resolved_plan,
        hooks: HookPlan {
            affected,
            active_owners,
            candidate_owners,
            pending,
        },
    })
}

/// Runs the affected config-reload hooks and then the decision gate against the candidate, in
/// `candidate_context`.
pub(super) async fn run_candidate_hooks(
    candidate_context: InvocationContext,
    hook_manager: &HookManager,
    staged: &StagedReload,
    plan: &HookPlan,
    recorder: &mut TransitionRecorder,
) -> crate::Result<()> {
    let hook_reports = candidate_context
        .clone()
        .enter(
            staged.run_config_reload_hooks(hook_manager, |owner| plan.candidate_owners.id(owner)),
        )
        .await?;
    let pending = plan.pending.decided(&hook_reports);

    recorder.record_hooks(hook_reports);
    recorder.enter(TransitionPhase::Decision);

    candidate_context
        .enter(fire_gate::<TransitionDecision>(
            hook_manager,
            &plan.candidate_owners,
            &pending,
            staged,
            &plan.affected,
            recorder,
        ))
        .await?;

    Ok(())
}
