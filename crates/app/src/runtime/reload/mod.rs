//! The transactional config-and-graph reload entry point.
//!
//! [`AppRuntime::reload_config`] re-reads configuration and republishes changed
//! bindings and the derived component graph as one serialized transaction. It stages
//! configuration, evaluates conditions, plans and builds a candidate graph and root over
//! the candidate [`ConfigStore`], and creates a generation-local [`HookManager`]. The
//! affected config-reload hooks run against that candidate generation before commit.
//!
//! Two pre-commit hook phases bracket construction: [`TransitionProposal`] hooks run
//! against the active generation after planning and before any construction, and
//! [`TransitionDecision`] hooks run against the candidate after the config-reload hooks,
//! immediately before commit. Either may reject the attempt; see
//! [`transition_hooks`](super::transition_hooks) for their routing and ordering.
//!
//! [`TransitionProposal`]: crate::TransitionProposal
//! [`TransitionDecision`]: crate::TransitionDecision
//!
//! # Rollback boundary
//!
//! Before the terminal commit, every recoverable error, hook rejection, failed candidate
//! factory, or cancellation leaves the framework-owned publication state unchanged: the
//! active runtime generation, the managed config tree and its live slots, the live cells
//! retained consumers hold, and the config generation. The candidate — its root, its hook
//! manager, and every newly constructed component instance — is dropped, so mutations held
//! exclusively by those candidate instances disappear with it. Retained dependencies may
//! alias the active generation's instances, so mutations a candidate factory or hook makes
//! through them reach live components and are not rolled back. Arbitrary external, global,
//! or I/O side effects (external systems, emitted messages, files) are likewise not rolled
//! back and are the application's responsibility.
//!
//! # Invalidation policy
//!
//! A non-empty reload reconstructs exactly the affected singletons in the candidate
//! root: consumers of changed config bindings, owners of [`ConfigReload`] hooks, and
//! consumers of the generation-local [`HookManager`], plus the singletons that depend on
//! them — the planner propagates each invalidation through reverse reachability.
//! Unaffected singletons retain their active instances. An affected singleton that
//! cannot reconstruct in place (no ordinary factory, …) fails the reload with
//! [`RestartRequired`](crate::RestartRequired) before any construction runs.
//!
//! # Scopes
//!
//! Affected scoped and transient components have no root instance, so they need no
//! strategy: the published scope plan rebuilds them on the next opening. Already open
//! scopes, and scopes opened beneath them, keep resolving from the generation they
//! pinned until they close; a reload never migrates or waits for them.
//!
//! # Live rebinding
//!
//! A retained singleton whose only changes are live (`Dep<T>`) dependencies keeps its
//! instance: the reload republishes the live cell it holds with the candidate's value
//! instead of reconstructing it, so its fixed dependents retain too. This applies to
//! single, eagerly resolved dependencies on singleton components or provider mappings
//! (including `Dep<dyn Trait>` provider switches) whose live cell the runtime can reach,
//! which every typed descriptor allows even when the target opts out of retention. Other
//! live consumers reconstruct and are displaced like replaced components.
//! Each consumer keeps the cell of the generation that constructed it for as long as it
//! is retained, and every rebind is prepared before commit and applied inside the
//! terminal publication. Snapshots taken from a rebound handle keep the previous
//! instance. The held cell is the constructing root's slot, so — as with [`Cfg<T>`] —
//! a pinned view of an earlier generation observes the new value through that slot once
//! a retained consumer rebinds it.
//!
//! # Transition handlers and hot updates
//!
//! A replaced or rebound singleton uses the most instance-preserving recipe its
//! component declares, in this order: an explicit hot update, a transition handler, a
//! replacement factory, then ordinary reconstruction.
//!
//! - A **transition handler** receives the component's planned change and returns a
//!   one-shot factory for this transition only, or declines so the next recipe applies.
//!   The one-shot factory runs at most once during candidate construction, may read the
//!   previous instance, and may resolve only dependencies the ordinary factory declares;
//!   any other dependency rejects the transition. It is dropped with its attempt, so a
//!   later transition asks the handler again.
//! - A **hot update** keeps the instance. Its preparation reads the instance and
//!   candidate dependencies after the candidate root is built and returns an owned
//!   commit action without mutating anything. The action applies during the terminal
//!   publication, after the live rebinds, and a failed or rejected attempt drops it
//!   unapplied. The action must not panic; one that does is contained and traced on
//!   `upwell::transition`, and the publication still completes. Only components stored
//!   as `Arc<Self>` can declare a hot update, so the action always reaches the retained
//!   instance itself. Because the instance is shared, every holder — snapshots, pinned views,
//!   and the retained live slot that keeps pointing at it — observes the update only
//!   once it commits. Hot updates apply only when the component's identity, factory and
//!   dependency contract are unchanged, and its fixed dependents still reconstruct as
//!   planned. A hot-updated component is not displaced, so it receives no
//!   [`ComponentRetired`](crate::ComponentRetired).
//!
//! # Reentrancy
//!
//! Transition hooks, config-reload hooks, and candidate factories run to completion
//! while both serialization leases — the config reloader's reload lock and the runtime
//! transition writer — are held, and neither lock is reentrant. The whole transaction
//! therefore runs inside a task-local active-transition marker: a hook or factory that
//! calls [`AppRuntime::reload_config`] or
//! [`RuntimeReloader::reload`](crate::builtins::RuntimeReloader::reload) on the same
//! runtime gets [`Error::ReentrantReload`](crate::Error::ReentrantReload) immediately,
//! without touching either lease, and the running reload keeps its normal outcome.
//! Markers nest across runtimes, so a cycle in which runtime A's transaction reloads
//! runtime B inline and B's transaction reloads A on the same task is rejected for A as
//! well. Cycles split across tasks (A's transaction reloading B while another task's B
//! transaction reloads A) are not detected and deadlock on each other's leases.
//!
//! The marker is task-local, so work a hook or factory explicitly `tokio::spawn`s does
//! not inherit it: a reload requested there waits for the lease and runs after the
//! transaction completes. Awaiting such a spawned reload from the hook or factory itself
//! still deadlocks, because the transaction cannot complete while it waits. Code that
//! awaits the [`ConfigReloader`](upwell_config::ConfigReloader) reload lease directly
//! bypasses the check.
//!
//! # Generation-bound hook managers
//!
//! Every generation owns its hook manager, and hook receivers resolve through their own
//! generation's root only. Refetch [`AppRuntime::hooks`] after a published reload to run
//! the new generation's hooks. A handle stays associated with its original generation's
//! hook catalog, but it can invoke only while that generation's root is still pinned or
//! owned — by a held [`RuntimeView`](crate::RuntimeView), the runtime's current
//! generation, or another root/scope/component owner. A standalone old handle whose root
//! has since been released returns
//! [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) outcomes instead of
//! invoking, so hold a pinned view across invocations that must pin one generation.
//! Components that inject [`HookManager`] are manager consumers and reconstruct
//! under the invalidation policy above.
//!
//! [`ConfigStore`]: upwell_config::ConfigStore
//!
//! Lock order is fixed: the config reloader's serialization lock is acquired first,
//! then the runtime transition writer, so the writer is never held while waiting on the
//! reloader.
//!
//! At the terminal commit, the validated candidate becomes the authoritative runtime
//! generation first, then the retained consumers' live cells are republished and the
//! live config slots synchronously commit while both leases remain held. The staged
//! commit advances the config generation exactly once per reload. An unchanged source is
//! a no-op: it moves neither generation nor root. A pinned
//! [`RuntimeView`](crate::RuntimeView) consistently pins one generation's root, graph,
//! scopes, condition state, and hook catalog, but it does not freeze separately held live
//! [`Cfg<T>`] cells. Those cells remain per-slot atomic at commit; no atomicity across
//! handles is promised.
//!
//! [`Cfg<T>`]: upwell_config::Cfg
//!
//! # Outcome observers
//!
//! After the attempt released both leases, observer-only
//! [`TransitionCommitted`](crate::TransitionCommitted) hooks fire on the published
//! generation for a committed attempt, and [`TransitionAborted`](crate::TransitionAborted)
//! hooks fire on the generation that stayed active for a rejected one, each exactly once
//! with the final report. They run outside the reentrancy scope, so they may request a
//! follow-up reload; their failures are traced and never affect the outcome.
//!
//! Before the committed observers, every singleton the commit displaced (constructed
//! again or removed, listed by [`RuntimeTransitionReport::displaced`]) receives
//! [`ComponentRetired`](crate::ComponentRetired) once, resolved through the displaced
//! generation's root so the receiver is the retired instance itself. Retirement is
//! logical displacement: pinned views and open scopes may keep using the instance until
//! they release it.
//!
//! # Reports
//!
//! Every reload that acquires the transition writer produces one canonical, redacted
//! [`RuntimeTransitionReport`] under a fresh [`TransitionAttemptId`](crate::TransitionAttemptId):
//! returned on commit and no-op, and carried by
//! [`Error::TransitionRejected`](crate::Error::TransitionRejected) on rejection. Each
//! report is also emitted as structured `upwell::transition` tracing events.
//!
//! This is the only reload pipeline. The injectable
//! [`RuntimeReloader`](crate::builtins::RuntimeReloader) drives
//! [`AppRuntime::reload_config`], and the app's watch and signal triggers (through
//! [`ReloadTarget`](upwell_config::ReloadTarget)) drive the same reload transaction,
//! recording their own [`ReloadCause`](upwell_config::ReloadCause).

use super::generation::RuntimeTransition;
use super::reentrancy::ActiveTransition;
use super::report::{
    ComponentTransitionReport, RuntimeTransitionReport, TRANSITION_TARGET, TransitionAttemptId,
    TransitionCompletion, TransitionPhase, TransitionRecorder,
};
use super::transition_hooks::{
    AffectedComponents, HookOwners, PendingTransition, TransitionDecision, TransitionProposal,
    fire_gate, observe_outcome,
};
use super::{AppRuntime, RuntimeView};
use stages::{commit_candidate, construct_candidate, evaluate_conditions, plan_transition};
#[cfg(feature = "tooling")]
pub(crate) use stages::{resolve_staged_transition, staged_condition_snapshot};
use upwell_config::ReloadCause;

impl AppRuntime {
    /// Re-reads configuration and republishes changed bindings and the derived
    /// component graph as one serialized transaction.
    ///
    /// It first acquires the [`ConfigReloader`](upwell_config::ConfigReloader) reload
    /// lock, then the runtime transition writer. While both leases are held, it stages
    /// configuration; evaluates conditions; plans the candidate graph; runs
    /// [`TransitionProposal`] hooks against the active
    /// generation; builds the candidate root over a candidate
    /// [`ConfigStore`](upwell_config::ConfigStore) with a generation-local
    /// [`HookManager`]; runs the affected config-reload hooks and then
    /// [`TransitionDecision`] hooks against that candidate; and
    /// commits. A proposal rejection constructs nothing; a decision rejection drops the
    /// constructed candidate.
    ///
    /// Once both leases are released, it retires the singletons a committed attempt
    /// displaced through [`ComponentRetired`](crate::ComponentRetired), then fires
    /// [`TransitionCommitted`](crate::TransitionCommitted) observers for a committed
    /// attempt or [`TransitionAborted`](crate::TransitionAborted) observers for a rejected
    /// one, then returns. The report's [`cause`](RuntimeTransitionReport::cause) is
    /// [`ReloadCause::Requested`]; `SIGHUP` and file-watch triggers record their own cause.
    ///
    /// Returns the attempt's [`RuntimeTransitionReport`] when it commits or is a no-op.
    /// A rejected attempt returns
    /// [`Error::TransitionRejected`](crate::Error::TransitionRejected), which carries the
    /// redacted report (rejecting phase and failure category) and the original error.
    ///
    /// Before the terminal commit, every recoverable error, hook rejection, failed
    /// candidate factory, or cancellation leaves the framework-owned publication state
    /// unchanged — the active runtime generation, the managed config tree and its live
    /// slots, and the config generation — and drops the candidate wholesale. Mutations
    /// held exclusively by newly constructed candidate instances disappear with that
    /// drop. Retained dependencies may alias the active generation's instances, so
    /// mutations made through them are not rolled back; arbitrary external, global, or
    /// I/O side effects are likewise not rolled back.
    ///
    /// Affected singletons reconstruct in the candidate root: consumers of changed
    /// config bindings, [`ConfigReload`](upwell_config::ConfigReload) hook owners, and
    /// [`HookManager`] consumers, plus the singletons that depend on them through the
    /// planner's reverse propagation. Unaffected singletons retain their active
    /// instances. An affected component that cannot reconstruct in place fails with
    /// [`RestartRequired`](crate::RestartRequired) before any construction runs.
    ///
    /// The terminal commit publishes the authoritative runtime generation first, then
    /// synchronously commits the live config slots while both leases remain held. The
    /// staged commit advances the config generation exactly once. This is the only
    /// reload pipeline: [`RuntimeReloader`](crate::builtins::RuntimeReloader) and the
    /// app's watch and signal triggers drive it.
    ///
    /// Hook managers are generation-bound: refetch [`hooks`](Self::hooks) after a
    /// published reload to run the new generation's hooks. A handle stays associated
    /// with its original generation but invokes only while that generation's root is
    /// still pinned or owned — a [`RuntimeView`] held across the
    /// invocation keeps it alive, while a standalone old handle may return
    /// [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) after the root
    /// is released. Components that inject [`HookManager`] reconstruct under the
    /// invalidation policy above.
    ///
    /// An unchanged source is a true no-op: no evaluation, construction, hooks, runtime
    /// or config generation movement, or root replacement. It still consumes an attempt
    /// ID and reports [`TransitionOutcome::NoOp`](crate::TransitionOutcome::NoOp). When a
    /// condition reads raw config paths, a reload without binding changes still
    /// re-evaluates conditions, and is a no-op only while eligibility is unchanged.
    ///
    /// # Reentrancy
    ///
    /// Transition hooks, config-reload hooks, and candidate factories run while both
    /// serialization leases are held, and neither lock is reentrant. Calling this method or
    /// [`RuntimeReloader::reload`](crate::builtins::RuntimeReloader::reload) for the
    /// same runtime from inside a running transaction returns
    /// [`Error::ReentrantReload`](crate::Error::ReentrantReload) instead of waiting on
    /// the leases the caller's own transaction holds; the running transaction is
    /// unaffected. Work explicitly `tokio::spawn`ed from a hook or factory does not
    /// inherit the marker and waits for the lease instead, so it must not be awaited
    /// from inside the transaction.
    ///
    /// [`HookManager`]: upwell_hooks::HookManager
    pub async fn reload_config(&self) -> crate::Result<RuntimeTransitionReport> {
        self.reload_for(ReloadCause::Requested).await
    }

    /// Runs one reload attempt requested by `cause`, then fires its outcome observers
    /// once both serialization leases are released.
    pub(crate) async fn reload_for(
        &self,
        cause: ReloadCause,
    ) -> crate::Result<RuntimeTransitionReport> {
        if let Some(attempt) = ActiveTransition::current_for(self.transitions.owner()) {
            return Err(reentrant_reload(attempt));
        }

        let reload_lease = self.reloader.lock_reload().await;
        let transition = self.begin_transition().await;
        let base = transition.base().clone();
        let active = transition.active();
        let mut recorder = TransitionRecorder::begin(
            transition.attempt(),
            cause,
            base.id(),
            self.reloader.generation(),
        );

        let result = active
            .scope(self.run_reload(transition, &mut recorder))
            .await;

        drop(reload_lease);

        let (result, published) = match result {
            Ok((completion, published)) => (Ok(completion), published),
            Err(error) => (Err(error), None),
        };
        let outcome = recorder.finish(result);

        observe_outcome(&outcome, &base, published.as_ref()).await;

        outcome
    }

    /// Runs one reload attempt's phases under both held leases, recording each phase and
    /// its redacted results, and returns the completion with the published view when it
    /// committed.
    ///
    /// Lock order is fixed by the caller: the config reloader's serialization lock first,
    /// then the runtime transition writer. An unchanged source is a true no-op, with no
    /// evaluation, construction, hooks, or generation movement. When conditions read raw
    /// config paths, a reload without binding changes still re-evaluates them and is a
    /// no-op only while eligibility is unchanged.
    async fn run_reload(
        &self,
        transition: RuntimeTransition,
        recorder: &mut TransitionRecorder,
    ) -> crate::Result<(TransitionCompletion, Option<RuntimeView>)> {
        let base = transition.base().clone();

        recorder.enter(TransitionPhase::Stage);

        let staged = self.reloader.stage()?;

        recorder.record_changed(staged.changed());

        if staged.is_empty() && !base.condition().evaluation().reads_raw_config() {
            transition.finish_noop();

            return Ok((TransitionCompletion::NoOp, None));
        }

        recorder.enter(TransitionPhase::Evaluate);

        let evaluation = evaluate_conditions(&base, &staged)?;

        if staged.is_empty()
            && evaluation
                .evaluation()
                .same_eligibility(base.condition().evaluation().evaluation())
        {
            transition.finish_noop();

            return Ok((TransitionCompletion::NoOp, None));
        }

        recorder.enter(TransitionPhase::Plan);

        let (candidate, resolved_plan) = plan_transition(&base, &staged, &evaluation)?;
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
        recorder.enter(TransitionPhase::Proposal);

        fire_gate::<TransitionProposal>(
            base.hooks(),
            &active_owners,
            &pending,
            &staged,
            &affected,
            recorder,
        )
        .await?;

        recorder.enter(TransitionPhase::Construct);

        let (prepared, hook_manager) =
            construct_candidate(&base, candidate, &resolved_plan, &staged, evaluation).await?;

        recorder.enter(TransitionPhase::Hooks);

        let hook_reports = staged
            .run_config_reload_hooks(&hook_manager, |owner| candidate_owners.id(owner))
            .await?;
        let pending = pending.decided(&hook_reports);

        recorder.record_hooks(hook_reports);
        recorder.enter(TransitionPhase::Decision);

        fire_gate::<TransitionDecision>(
            &hook_manager,
            &candidate_owners,
            &pending,
            &staged,
            &affected,
            recorder,
        )
        .await?;

        recorder.enter(TransitionPhase::Commit);

        let (completion, published) = commit_candidate(transition, prepared, staged);

        Ok((completion, Some(published)))
    }
}

/// Rejects a reload nested inside the running `attempt` of the same runtime, and traces
/// the rejection.
fn reentrant_reload(attempt: TransitionAttemptId) -> crate::Error {
    tracing::warn!(
        target: TRANSITION_TARGET,
        attempt = attempt.get(),
        outcome = "reentrant",
        "reload requested from inside the running transition of the same runtime"
    );

    crate::Error::ReentrantReload { attempt }
}

mod invalidation;
mod stages;
mod trigger;

#[cfg(test)]
mod tests;
