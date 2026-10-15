use std::any::TypeId;
use std::collections::HashSet;

use upwell_config::{ReloadProposal, StagedReload};
use upwell_di::ComponentDescriptor;
use upwell_hooks::{HookDescriptor, HookKind, HookManager, HookRole};

use super::{HookOwners, PendingTransition, TransitionHookContext, TransitionVerdict};
use crate::ComponentTransitionReport;
use crate::runtime::report::{
    TRANSITION_TARGET, TransitionHookOutcome, TransitionHookReport, TransitionRecorder,
};

/// The component types a transition plan affects, which targeted transition participants
/// are routed to.
pub(crate) struct AffectedComponents {
    types: HashSet<TypeId>,
}

impl AffectedComponents {
    /// Resolves the affected stable component IDs to their types through the active and
    /// candidate descriptors, so removed and added components are both covered.
    pub(crate) fn new(
        components: &[ComponentTransitionReport],
        active: &[ComponentDescriptor],
        candidate: &[ComponentDescriptor],
    ) -> Self {
        let ids = components
            .iter()
            .map(|component| component.component)
            .collect::<HashSet<_>>();
        let types = active
            .iter()
            .chain(candidate)
            .filter(|descriptor| ids.contains(descriptor.id))
            .map(|descriptor| descriptor.ty.type_id)
            .collect();

        Self { types }
    }

    fn targets(&self, hook: &HookDescriptor) -> bool {
        self.types.contains(&hook.component_ty.type_id)
    }
}

/// Fires one pre-commit phase of kind `K` against `hooks`, records every hook that ran
/// under its owner's stable ID from `owners`, and returns the error of the participant
/// that ended the attempt, if any.
pub(crate) async fn fire_gate<K>(
    hooks: &HookManager,
    owners: &HookOwners,
    pending: &PendingTransition,
    staged: &StagedReload,
    affected: &AffectedComponents,
    recorder: &mut TransitionRecorder,
) -> crate::Result<()>
where
    K: HookKind<Output = TransitionVerdict, Cx = TransitionHookContext>,
{
    if !hooks.has::<K>() {
        return Ok(());
    }

    let phase = pending.phase();
    let cx = TransitionHookContext::new(
        pending.clone(),
        ReloadProposal::new(staged.staged().to_vec()),
    );
    let fired = hooks.fire::<K>(&cx, |hook| affected.targets(hook)).await;
    let mut reports = Vec::with_capacity(fired.participants.len() + fired.observers.len());
    let mut rejection = None;

    for (owner, outcome) in fired.participants {
        let component = owners.id(&owner);
        let (outcome, error) = participant_outcome::<K>(component, outcome);

        reports.push(TransitionHookReport {
            phase,
            component,
            role: HookRole::Participant,
            outcome,
        });
        rejection = error.or(rejection);
    }

    for (owner, outcome) in fired.observers {
        let component = owners.id(&owner);
        let outcome = observer_outcome(outcome);

        if outcome != TransitionHookOutcome::Completed {
            tracing::warn!(
                target: TRANSITION_TARGET,
                attempt = recorder.attempt().get(),
                phase = phase.as_str(),
                hook = K::NAME,
                component,
                outcome = ?outcome,
                "transition observer failed"
            );
        }

        reports.push(TransitionHookReport {
            phase,
            component,
            role: HookRole::Observer,
            outcome,
        });
    }

    recorder.record_transition_hooks(reports);

    rejection.map_or(Ok(()), Err)
}

/// Classifies a participant's outcome for the report and, when it ended the attempt,
/// builds the typed error that rejects it.
fn participant_outcome<K: HookKind>(
    component: &'static str,
    outcome: upwell_hooks::Result<TransitionVerdict>,
) -> (TransitionHookOutcome, Option<crate::Error>) {
    match outcome {
        Ok(TransitionVerdict::Accept) => (TransitionHookOutcome::Completed, None),
        Ok(TransitionVerdict::Reject(reason)) => (
            TransitionHookOutcome::Rejected { reason },
            Some(crate::Error::TransitionHookRejected {
                kind: K::NAME,
                component,
                reason,
            }),
        ),
        Err(error) => (
            failure_outcome(&error),
            Some(crate::Error::TransitionHookFailed {
                kind: K::NAME,
                component,
                source: Box::new(error),
            }),
        ),
    }
}

fn observer_outcome(outcome: upwell_hooks::Result<()>) -> TransitionHookOutcome {
    match outcome {
        Ok(()) => TransitionHookOutcome::Completed,
        Err(error) => failure_outcome(&error),
    }
}

fn failure_outcome(error: &upwell_hooks::Error) -> TransitionHookOutcome {
    match error {
        upwell_hooks::Error::Panicked { .. } => TransitionHookOutcome::Panicked,
        _ => TransitionHookOutcome::Failed,
    }
}
