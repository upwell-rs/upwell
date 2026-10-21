//! The stages of one reload transaction that run under both serialization leases:
//! condition evaluation, transition planning, candidate construction, and commit.

use std::cell::Cell;
use std::sync::Arc;

use upwell_config::StagedReload;
use upwell_core::{ResolverSet, TypeDescriptor};
use upwell_di::{BoxedComponent, Injectable};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookManager};

use super::super::generation::RuntimeTransition;
use super::super::report::TransitionCompletion;
use super::super::{AppConditionState, PreparedRuntimeGeneration, RuntimeView};
use crate::registry::AppConditionEvaluation;
use crate::transition::{CandidateGraph, CommitActions, ResolvedTransitionPlan};

/// Snapshots the staged condition facts and incrementally re-evaluates conditions against
/// the base generation's catalog, which was validated once at startup: fact-descriptor
/// callbacks do not run again here.
///
/// The fact-extraction and condition callbacks are application extension points, so the
/// whole stage runs inside a panic boundary: a panic returns the redacted
/// [`CandidatePreparationPanicked`](crate::Error::CandidatePreparationPanicked) error and
/// aborts the reload before any candidate graph, root, hook, or publication work.
pub(super) fn evaluate_conditions(
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
                .map(|entry| (entry.type_id(), entry.path(), entry.shared_value())),
            |path| staged.resolve_raw(path),
        )?;

        condition.evaluation().reevaluate(&snapshot)
    }))
    .map_err(|_| crate::Error::CandidatePreparationPanicked)??;

    Ok(evaluation)
}

/// Prepares and validates the candidate graph from the re-evaluated eligibility, then
/// resolves the runtime transition against the base generation's effective graph.
///
/// When re-evaluation leaves eligibility unchanged, the candidate graph would equal the
/// base's, so the base graph and scope plans are shared instead of rebuilt; `evaluation`
/// was re-evaluated against the base's own catalog, so equal eligibility means an equal
/// effective registry. Planning still runs, so changed bindings invalidate as usual.
///
/// The deterministic invalidation roots — changed config bindings, matched by exact
/// `(TypeId, path)` identity, config-reload hook owners, and `HookManager` consumers —
/// must reconstruct instead of retaining stale state. The planner propagates each
/// invalidation to dependent singletons; an affected component that cannot reconstruct
/// in place fails resolution with `RestartRequired` before any construction runs.
pub(super) fn plan_transition(
    base: &RuntimeView,
    staged: &StagedReload,
    evaluation: &AppConditionEvaluation,
) -> crate::Result<(CandidateGraph, ResolvedTransitionPlan)> {
    let catalog = base.condition().catalog();
    let topology = &base.scope_plan().topology;

    let candidate = if evaluation
        .evaluation()
        .same_eligibility(base.condition().evaluation().evaluation())
    {
        CandidateGraph::reuse(base)
    } else {
        CandidateGraph::prepare_evaluation(base.id(), catalog, evaluation, topology)?
    };
    let changed_typed = staged
        .changed_staged()
        .map(|entry| (entry.type_id(), entry.path()))
        .collect::<Vec<_>>();
    let invalidated = super::invalidation::invalidated_component_roots(&candidate, &changed_typed);
    let resolved_plan =
        candidate.resolve_runtime_transition(base.effective_graph(), &invalidated)?;

    Ok((candidate, resolved_plan))
}

/// Builds the candidate root and the prepared generation, plus the generation-local hook
/// manager built from the candidate descriptors.
///
/// The prepared generation also carries the live rebinds for retained consumers whose
/// live dependencies changed and the prepared hot updates; they apply only inside its
/// terminal publication.
///
/// The candidate root seeds that manager through a generation override, so candidate
/// hooks resolve through the candidate root only and the active manager is never reused.
pub(super) async fn construct_candidate(
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
    let (held_live_cells, live_rebinds) =
        resolved_plan.prepare_live_rebinds(&candidate, base, &root)?;
    let hot_updates = resolved_plan.prepare_hot_updates(&candidate, &root).await?;
    let (resolved, scope_plan, graph) = candidate.into_runtime_parts(&topology);
    let prepared = PreparedRuntimeGeneration::new(
        root,
        scopes,
        scope_plan,
        resolved,
        graph,
        Arc::new(AppConditionState::new(Arc::clone(catalog), evaluation)),
    )
    .with_commit_actions(
        held_live_cells,
        CommitActions::new(live_rebinds, hot_updates),
    );

    Ok((prepared, hook_manager))
}

/// Publishes the validated candidate and commits the staged config slots, returning the
/// completion and the published view.
///
/// The validated candidate becomes current first, then the staged config slots commit
/// while the writer lease is still held, advancing the config generation exactly once.
/// Every step is infallible: under the held writer lease the base cannot have moved, so
/// `prepare_commit` cannot report a stale proposal.
pub(super) fn commit_candidate(
    transition: RuntimeTransition,
    prepared: PreparedRuntimeGeneration,
    staged: StagedReload,
) -> (TransitionCompletion, RuntimeView) {
    let config_generation = Cell::new(0);
    let commit = transition
        .prepare_commit(prepared)
        .expect("candidate prepared under the sole writer cannot be stale");
    let view = commit.commit_with(|| {
        config_generation.set(staged.commit());
    });

    let completion = TransitionCompletion::Committed {
        generation: view.id(),
        config_generation: config_generation.get(),
    };

    (completion, view)
}
