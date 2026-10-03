//! The transactional config-and-graph reload entry point.
//!
//! [`AppRuntime::reload_config`] re-reads configuration and republishes changed
//! bindings and the derived component graph as one serialized transaction. It stages
//! configuration, evaluates conditions, plans and builds a candidate graph and root over
//! the candidate [`ConfigStore`], and creates a generation-local [`HookManager`]. The
//! affected config-reload hooks run against that candidate generation before commit. Any
//! recoverable error, hook rejection, or cancellation before the terminal commit preserves
//! the complete active state, including configuration values.
//!
//! [`ConfigStore`]: upwell_config::ConfigStore
//!
//! Lock order is fixed: the config reloader's serialization lock is acquired first,
//! then the runtime transition writer. A legacy [`ConfigReloader::reload`] holds only
//! the first lock, so it can never interleave with this transaction, and the writer is
//! never held while waiting on the reloader.
//!
//! At the terminal commit, the validated candidate becomes the authoritative runtime
//! generation first, then the legacy live config slots synchronously commit while both
//! leases remain held, and the config reloader's compatibility hook manager is replaced
//! with the published generation's. An unchanged source is a no-op: it moves neither
//! generation nor root. A pinned [`RuntimeView`](crate::RuntimeView) consistently pins
//! one generation's root, graph, scopes, condition state, and hook catalog, but it does
//! not freeze separately held legacy live [`Cfg<T>`] cells. Those cells remain per-slot
//! atomic at commit; no atomicity across handles is promised.
//!
//! [`Cfg<T>`]: upwell_config::Cfg
//!
//! [`ConfigReloader::reload`] remains a config-only compatibility operation: it runs the
//! current generation's hooks against the current root, but does not transition the
//! graph. Watch and signal triggers also continue to call it until #211; application
//! graph transitions must use [`AppRuntime::reload_config`].

use std::cell::Cell;
use std::sync::Arc;

use upwell_config::{ChangedBinding, ComponentHookReport};
use upwell_core::{ResolverSet, RuntimeGenerationId, TypeDescriptor};
use upwell_di::{BoxedComponent, Injectable};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookManager};

use super::{AppConditionState, AppRuntime, PreparedRuntimeGeneration};
use crate::transition::CandidateGraph;

/// The outcome of one transactional config-and-graph reload.
#[derive(Debug, Clone)]
pub struct RuntimeReloadReport {
    /// The runtime generation current after the reload — unchanged when nothing was
    /// published.
    pub runtime_generation: RuntimeGenerationId,
    /// The config generation after the reload — unchanged when the source was
    /// unchanged.
    pub config_generation: u64,
    /// The bindings whose source changed and were re-published. Empty for an
    /// unchanged-source no-op.
    pub changed: Vec<ChangedBinding>,
    /// The config-reload hooks that ran and accepted, with their outcomes.
    pub hooks: Vec<ComponentHookReport>,
    /// Whether a new runtime generation was published. `false` for an unchanged-source
    /// no-op, which leaves every generation and the root untouched.
    pub published: bool,
}

impl AppRuntime {
    /// Re-reads configuration and republishes changed bindings and the derived
    /// component graph as one serialized transaction.
    ///
    /// It first acquires the [`ConfigReloader`] reload lock, then the runtime transition
    /// writer. While both leases are held, it stages configuration; evaluates conditions;
    /// plans and builds a candidate graph and root over a candidate
    /// [`ConfigStore`](upwell_config::ConfigStore); and
    /// creates a generation-local [`HookManager`]. Affected config-reload hooks run
    /// against that candidate before commit. Any recoverable error, hook rejection, or
    /// cancellation before the terminal commit preserves the complete active state,
    /// including configuration values.
    ///
    /// The terminal commit publishes the authoritative runtime generation first, then
    /// synchronously commits legacy live config slots and installs the published
    /// generation's hook manager into the [`ConfigReloader`] while both leases remain
    /// held — so legacy reloads run the current generation's hooks against the current
    /// root. Unlike [`ConfigReloader::reload`], which is config-only compatibility
    /// behavior, this method is required for application graph transitions. Watch and
    /// signal triggers continue to use `ConfigReloader::reload` until #211.
    ///
    /// An unchanged source is a true no-op: no evaluation, construction, hooks, runtime
    /// or config generation movement, or root replacement.
    pub async fn reload_config(&self) -> crate::Result<RuntimeReloadReport> {
        // Fixed lock order: the config reloader's serialization lock first, then the
        // runtime transition writer.
        let _reload_lease = self.reloader.lock_reload().await;
        let transition = self.begin_transition().await;
        let base = transition.base().clone();

        // Stage: re-read, diff, and deserialize changed bindings into committable swaps
        // plus a generation-local candidate store. Nothing is published yet.
        let staged = self.reloader.stage()?;
        let changed = staged.changed().to_vec();

        // Unchanged source: a true no-op. No evaluation, no construction, no hooks, and
        // no generation movement — the pinned base stays current.
        if staged.is_empty() {
            let view = transition.finish_noop();

            return Ok(RuntimeReloadReport {
                runtime_generation: view.id(),
                config_generation: self.reloader.generation(),
                changed,
                hooks: Vec::new(),
                published: false,
            });
        }

        // Condition snapshot and incremental re-evaluation from the staged values
        // against the base generation's catalog.
        let condition = base.condition();
        let catalog = condition.catalog();
        let snapshot = catalog.condition_snapshot(
            staged
                .staged()
                .iter()
                .map(|entry| (entry.type_id(), entry.path(), entry.value())),
        )?;
        let facts = catalog
            .condition_facts
            .iter()
            .flat_map(|source| (source.facts.descriptors)());
        let evaluation =
            catalog.evaluate_changed_conditions(facts, condition.evaluation(), &snapshot)?;

        // Prepare and validate the candidate graph from the re-evaluated eligibility.
        let topology = Arc::clone(&base.scope_plan().topology);
        let candidate =
            CandidateGraph::prepare_evaluation(base.id(), catalog, &evaluation, &topology)?;
        let resolved_plan = candidate.resolve_runtime_transition(base.effective_graph())?;

        // A generation-local hook manager built from the candidate descriptors. The
        // candidate root seeds it through the generation override below, so candidate
        // hooks resolve through the candidate root only — the active manager is never
        // reused.
        let hooks: Vec<HookDescriptor> = candidate
            .components()
            .iter()
            .flat_map(|component| (component.hooks)().iter().copied())
            .collect();
        let hook_manager = HookManager::new(hooks);

        let mut externals = ResolverSet::new();
        externals.insert(staged.candidate_store());

        let overrides = vec![BoxedComponent {
            ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
            value: Box::new(Injectable::into_stored(hook_manager.clone())),
        }];
        let (root, scopes) = resolved_plan
            .build_candidate_root_from_active_with_overrides(
                &candidate, &base, externals, overrides,
            )
            .await?;

        // Attach the candidate root as the candidate manager's resolver context and
        // assemble the prepared generation from the candidate's parts.
        let (resolved, scope_plan, graph) = candidate.into_runtime_parts(&topology);
        let prepared = PreparedRuntimeGeneration::new(
            root,
            scopes,
            scope_plan,
            resolved,
            graph,
            Arc::new(AppConditionState::new(Arc::clone(catalog), evaluation)),
        );

        // Run the filtered config-reload hooks against the candidate manager and the
        // staged proposal. Any rejection aborts before the commit token is consumed.
        let hook_reports = staged.run_config_reload_hooks(&hook_manager).await?;

        // Prepare the commit token: validates the candidate against the exact base.
        // Under the held writer lease the base cannot have moved, so this cannot fail.
        let commit = transition
            .prepare_commit(prepared)
            .expect("candidate prepared under the sole writer cannot be stale");

        // Terminal publication: the validated candidate becomes current first, then the
        // staged config slots commit while the writer lease is still held. From the
        // commit token on, every step is infallible.
        let config_generation = Cell::new(0);
        let view = commit.commit_with(|| {
            let generation = staged.commit();

            // Install the just-published generation's hook manager — the same manager
            // seeded in the candidate root — while both serializers remain held, so
            // legacy reloads (direct calls and watch/signal triggers) run the current
            // generation's hooks against the current root.
            self.reloader.install_hook_manager(hook_manager);

            config_generation.set(generation);
        });

        Ok(RuntimeReloadReport {
            runtime_generation: view.id(),
            config_generation: config_generation.get(),
            changed,
            hooks: hook_reports,
            published: true,
        })
    }
}

#[cfg(test)]
mod tests;
