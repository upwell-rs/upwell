//! The transactional config-and-graph reload entry point.
//!
//! [`AppRuntime::reload_config`] re-reads configuration and republishes changed
//! bindings and the derived component graph as one serialized transaction. It stages
//! configuration, evaluates conditions, plans and builds a candidate graph and root over
//! the candidate [`ConfigStore`], and creates a generation-local [`HookManager`]. The
//! affected config-reload hooks run against that candidate generation before commit.
//!
//! # Rollback boundary
//!
//! Before the terminal commit, every recoverable error, hook rejection, failed candidate
//! factory, or cancellation leaves the framework-owned publication state unchanged: the
//! active runtime generation, the managed config tree and its live slots, and the config
//! generation. The candidate — its root, its hook manager, and every newly constructed component
//! instance — is dropped, so mutations held exclusively by those candidate instances
//! disappear with it. Retained dependencies may alias the active generation's
//! instances, so mutations a candidate factory or hook makes through them reach live
//! components and are not rolled back. Arbitrary external, global, or I/O side effects
//! (external systems, emitted messages, files) are likewise not rolled back and are the
//! application's responsibility.
//!
//! # Invalidation policy
//!
//! A non-empty reload reconstructs exactly the affected singletons in the candidate
//! root: consumers of changed config bindings, owners of [`ConfigReload`] hooks, and
//! consumers of the generation-local [`HookManager`], plus the singletons that depend on
//! them — the planner propagates each invalidation through reverse reachability.
//! Unaffected singletons retain their active instances. An affected component that
//! cannot reconstruct in place (no ordinary factory, non-singleton, …) fails the reload
//! with [`RestartRequired`](crate::RestartRequired) before any construction runs.
//!
//! # Reentrancy
//!
//! Config-reload hooks and candidate factories run to completion while both
//! serialization leases — the config reloader's reload lock and the runtime
//! transition writer — are held, and neither lock is reentrant. The whole transaction
//! therefore runs inside a task-local active-transition marker: a hook or factory that
//! calls [`AppRuntime::reload_config`] or
//! [`RuntimeReloader::reload`](crate::builtins::RuntimeReloader::reload) on the same
//! runtime gets [`Error::ReentrantReload`](crate::Error::ReentrantReload) immediately,
//! without touching either lease, and the running reload keeps its normal outcome.
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
//! generation first, then the live config slots synchronously commit while both leases
//! remain held. The staged commit advances the config generation
//! exactly once per reload. An unchanged source is a no-op: it moves neither generation
//! nor root. A pinned [`RuntimeView`](crate::RuntimeView) consistently pins one
//! generation's root, graph, scopes, condition state, and hook catalog, but it does not
//! freeze separately held live [`Cfg<T>`] cells. Those cells remain per-slot
//! atomic at commit; no atomicity across handles is promised.
//!
//! [`Cfg<T>`]: upwell_config::Cfg
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
//! [`RuntimeReloader`](crate::builtins::RuntimeReloader) and the app's watch and signal
//! triggers (through [`ReloadTarget`](upwell_config::ReloadTarget)) all drive
//! [`AppRuntime::reload_config`].

use std::cell::Cell;
use std::sync::Arc;

use upwell_config::StagedReload;
use upwell_core::{ResolverSet, TypeDescriptor};
use upwell_di::{BoxedComponent, Injectable};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookManager};

use super::generation::RuntimeTransition;
use super::reentrancy::ActiveTransition;
use super::report::{
    ComponentTransitionReport, RuntimeTransitionReport, TRANSITION_TARGET, TransitionAttemptId,
    TransitionCompletion, TransitionPhase, TransitionRecorder,
};
use super::{AppConditionState, AppRuntime, PreparedRuntimeGeneration, RuntimeView};
use crate::registry::AppConditionEvaluation;
use crate::transition::{CandidateGraph, ResolvedTransitionPlan};

impl AppRuntime {
    /// Re-reads configuration and republishes changed bindings and the derived
    /// component graph as one serialized transaction.
    ///
    /// It first acquires the [`ConfigReloader`](upwell_config::ConfigReloader) reload
    /// lock, then the runtime transition writer. While both leases are held, it stages
    /// configuration; evaluates conditions; plans and builds a candidate graph and root
    /// over a candidate [`ConfigStore`](upwell_config::ConfigStore); and
    /// creates a generation-local [`HookManager`]. Affected config-reload hooks run
    /// against that candidate before commit.
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
    /// still pinned or owned — a [`RuntimeView`](crate::RuntimeView) held across the
    /// invocation keeps it alive, while a standalone old handle may return
    /// [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) after the root
    /// is released. Components that inject [`HookManager`] reconstruct under the
    /// invalidation policy above.
    ///
    /// An unchanged source is a true no-op: no evaluation, construction, hooks, runtime
    /// or config generation movement, or root replacement. It still consumes an attempt
    /// ID and reports [`TransitionOutcome::NoOp`](crate::TransitionOutcome::NoOp).
    ///
    /// # Reentrancy
    ///
    /// Config-reload hooks and candidate factories run while both serialization
    /// leases are held, and neither lock is reentrant. Calling this method or
    /// [`RuntimeReloader::reload`](crate::builtins::RuntimeReloader::reload) for the
    /// same runtime from inside a running transaction returns
    /// [`Error::ReentrantReload`](crate::Error::ReentrantReload) instead of waiting on
    /// the leases the caller's own transaction holds; the running transaction is
    /// unaffected. Work explicitly `tokio::spawn`ed from a hook or factory does not
    /// inherit the marker and waits for the lease instead, so it must not be awaited
    /// from inside the transaction.
    pub async fn reload_config(&self) -> crate::Result<RuntimeTransitionReport> {
        if let Some(attempt) = ActiveTransition::current_for(self.transitions.owner()) {
            return Err(reentrant_reload(attempt));
        }

        let _reload_lease = self.reloader.lock_reload().await;
        let transition = self.begin_transition().await;
        let active = transition.active();
        let mut recorder = TransitionRecorder::begin(
            transition.attempt(),
            transition.base().id(),
            self.reloader.generation(),
        );

        let result = active
            .scope(self.run_reload(transition, &mut recorder))
            .await;

        recorder.finish(result)
    }

    /// Runs one reload attempt's phases under both held leases, recording each phase and
    /// its redacted results.
    ///
    /// Lock order is fixed by the caller: the config reloader's serialization lock first,
    /// then the runtime transition writer. An unchanged source is a true no-op, with no
    /// evaluation, construction, hooks, or generation movement.
    async fn run_reload(
        &self,
        transition: RuntimeTransition,
        recorder: &mut TransitionRecorder,
    ) -> crate::Result<TransitionCompletion> {
        let base = transition.base().clone();

        recorder.enter(TransitionPhase::Stage);

        let staged = self.reloader.stage()?;

        recorder.record_changed(staged.changed());

        if staged.is_empty() {
            transition.finish_noop();

            return Ok(TransitionCompletion::NoOp);
        }

        recorder.enter(TransitionPhase::Evaluate);

        let evaluation = evaluate_conditions(&base, &staged)?;

        recorder.enter(TransitionPhase::Plan);

        let (candidate, resolved_plan) = plan_transition(&base, &staged, &evaluation)?;

        recorder.record_components(ComponentTransitionReport::project(
            resolved_plan.structural(),
            base.effective_graph(),
            candidate.graph(),
        ));
        recorder.enter(TransitionPhase::Construct);

        let (prepared, hook_manager) =
            construct_candidate(&base, candidate, &resolved_plan, &staged, evaluation).await?;

        recorder.enter(TransitionPhase::Hooks);

        let hook_reports = staged.run_config_reload_hooks(&hook_manager).await?;

        recorder.record_hooks(hook_reports);
        recorder.enter(TransitionPhase::Commit);

        let completion = commit_candidate(transition, prepared, staged);

        Ok(completion)
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

/// Snapshots the staged condition facts and incrementally re-evaluates conditions against
/// the base generation's catalog.
///
/// The fact-extraction, descriptor, and condition callbacks are application extension
/// points, so the whole stage runs inside a panic boundary: a panic returns the redacted
/// [`CandidatePreparationPanicked`](crate::Error::CandidatePreparationPanicked) error and
/// aborts the reload before any candidate graph, root, hook, or publication work.
fn evaluate_conditions(
    base: &RuntimeView,
    staged: &StagedReload,
) -> crate::Result<AppConditionEvaluation> {
    let condition = base.condition();
    let catalog = condition.catalog();

    let evaluation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let snapshot = catalog.condition_snapshot(
            staged
                .staged()
                .iter()
                .map(|entry| (entry.type_id(), entry.path(), entry.value())),
        )?;
        let facts = catalog
            .condition_facts
            .iter()
            .flat_map(|source| (source.facts.descriptors)(source.path));

        catalog.evaluate_changed_conditions(facts, condition.evaluation(), &snapshot)
    }))
    .map_err(|_| crate::Error::CandidatePreparationPanicked)??;

    Ok(evaluation)
}

/// Prepares and validates the candidate graph from the re-evaluated eligibility, then
/// resolves the runtime transition against the base generation's effective graph.
///
/// The deterministic invalidation roots — changed config bindings, matched by exact
/// `(TypeId, path)` identity, config-reload hook owners, and `HookManager` consumers —
/// must reconstruct instead of retaining stale state. The planner propagates each
/// invalidation to dependent singletons; an affected component that cannot reconstruct
/// in place fails resolution with `RestartRequired` before any construction runs.
fn plan_transition(
    base: &RuntimeView,
    staged: &StagedReload,
    evaluation: &AppConditionEvaluation,
) -> crate::Result<(CandidateGraph, ResolvedTransitionPlan)> {
    let catalog = base.condition().catalog();
    let topology = &base.scope_plan().topology;

    let candidate = CandidateGraph::prepare_evaluation(base.id(), catalog, evaluation, topology)?;
    let changed_typed = staged
        .changed_staged()
        .map(|entry| (entry.type_id(), entry.path()))
        .collect::<Vec<_>>();
    let invalidated = invalidation::invalidated_component_roots(&candidate, &changed_typed);
    let resolved_plan =
        candidate.resolve_runtime_transition(base.effective_graph(), &invalidated)?;

    Ok((candidate, resolved_plan))
}

/// Builds the candidate root and the prepared generation, plus the generation-local hook
/// manager built from the candidate descriptors.
///
/// The candidate root seeds that manager through a generation override, so candidate
/// hooks resolve through the candidate root only and the active manager is never reused.
async fn construct_candidate(
    base: &RuntimeView,
    candidate: CandidateGraph,
    resolved_plan: &ResolvedTransitionPlan,
    staged: &StagedReload,
    evaluation: AppConditionEvaluation,
) -> crate::Result<(PreparedRuntimeGeneration, HookManager)> {
    let catalog = base.condition().catalog();
    let topology = Arc::clone(&base.scope_plan().topology);
    let hooks: Vec<HookDescriptor> = candidate
        .components()
        .iter()
        .flat_map(|component| (component.hooks)().iter().copied())
        .collect();
    let hook_manager = HookManager::new(hooks);
    let mut externals = ResolverSet::new();
    let overrides = vec![BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(hook_manager.clone())),
    }];

    externals.insert(staged.candidate_store());

    let (root, scopes) = resolved_plan
        .build_candidate_root_from_active_with_overrides(&candidate, base, externals, overrides)
        .await?;
    let (resolved, scope_plan, graph) = candidate.into_runtime_parts(&topology);
    let prepared = PreparedRuntimeGeneration::new(
        root,
        scopes,
        scope_plan,
        resolved,
        graph,
        Arc::new(AppConditionState::new(Arc::clone(catalog), evaluation)),
    );

    Ok((prepared, hook_manager))
}

/// Publishes the validated candidate and commits the staged config slots.
///
/// The validated candidate becomes current first, then the staged config slots commit
/// while the writer lease is still held, advancing the config generation exactly once.
/// Every step is infallible: under the held writer lease the base cannot have moved, so
/// `prepare_commit` cannot report a stale proposal.
fn commit_candidate(
    transition: RuntimeTransition,
    prepared: PreparedRuntimeGeneration,
    staged: StagedReload,
) -> TransitionCompletion {
    let config_generation = Cell::new(0);
    let commit = transition
        .prepare_commit(prepared)
        .expect("candidate prepared under the sole writer cannot be stale");
    let view = commit.commit_with(|| {
        config_generation.set(staged.commit());
    });

    TransitionCompletion::Committed {
        generation: view.id(),
        config_generation: config_generation.get(),
    }
}

mod invalidation;
mod trigger;

#[cfg(test)]
mod tests;
