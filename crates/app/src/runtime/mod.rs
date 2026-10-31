//! The protocol-facing runtime handle.
//!
//! [`AppRuntime`] is the cheap-clone handle a [`ProtocolRuntime`](crate::ProtocolRuntime)
//! receives to drive requests through the DI container and reach the app's support
//! systems. It owns the *agnostic* runtime state — the built scope containers and the
//! per-scope construction orders — that the serve loop used to take as a long argument
//! list, and exposes the scope-opening primitives a protocol drives per connection and
//! per request. Each committed runtime generation carries its own hook manager.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_config::ConfigReloader;
use upwell_core::{RuntimeGenerationId, ScopeId};
use upwell_di::{ComponentDescriptor, ScopeContainer};
use upwell_hooks::HookManager;

use crate::entry::EntryTable;
use crate::scope::{PreparedScopeTopology, ScopeLayouts, SeedDestination};

mod generation;
mod opener;
mod reentrancy;
mod reload;
mod report;
mod transition_hooks;

pub(crate) use generation::PreparedRuntimeGeneration;
pub use generation::{AppConditionState, DisplacedGeneration, RuntimeView};
use generation::{RuntimeGeneration, RuntimeTransitionCoordinator, WeakTransitionCoordinator};
pub use opener::{OpenScope, OpensUnder, ScopeBuilder, ScopeEntry, ScopeOpener};
#[cfg(feature = "tooling")]
pub(crate) use reload::{resolve_staged_transition, staged_condition_snapshot};
pub(crate) use report::TRANSITION_TARGET;
pub use report::{
    ComponentTransitionReport, PhaseTiming, RuntimeTransitionReport, TransitionAttemptId,
    TransitionFailure, TransitionHookOutcome, TransitionHookReport, TransitionOutcome,
    TransitionPhase, TransitionRejection,
};
pub use transition_hooks::{
    ComponentRetired, ComponentRetirement, PendingTransition, RetirementContext, RetirementReason,
    TransitionAborted, TransitionCommitted, TransitionDecision, TransitionHookContext,
    TransitionOutcomeContext, TransitionProposal, TransitionVerdict,
};

/// Everything a protocol needs to drive requests through DI, cheaply cloneable.
///
/// Agnostic to any particular protocol: it holds the built root scope, the per-scope
/// construction orders keyed by stable scope identity, the prepared protocol-owned
/// topology, and the resolved component set. The hook manager is generation-local and
/// read from the pinned current view. A protocol opens its declared boundaries through
/// [`ScopeOpener`]s it prepares with [`opener`](Self::opener).
#[derive(Clone)]
pub struct AppRuntime {
    name: Arc<str>,
    transitions: RuntimeTransitionCoordinator,
    /// The config reloader this runtime's transactional reload drives. Shared with the
    /// app handle, so its serialization lock covers both pipelines.
    reloader: ConfigReloader,
    /// The application's prepared entry points, fixed for its lifetime.
    entries: Arc<EntryTable>,
}

/// A non-owning [`AppRuntime`] reference, so framework handles seeded into the runtime's
/// own root never keep that runtime alive.
#[derive(Clone)]
pub(crate) struct WeakAppRuntime {
    name: Arc<str>,
    transitions: WeakTransitionCoordinator,
    reloader: ConfigReloader,
    entries: Arc<EntryTable>,
}

impl WeakAppRuntime {
    /// The runtime, while any owner still holds it.
    pub(crate) fn upgrade(&self) -> Option<AppRuntime> {
        let transitions = self.transitions.upgrade()?;

        Some(AppRuntime {
            name: Arc::clone(&self.name),
            transitions,
            reloader: self.reloader.clone(),
            entries: Arc::clone(&self.entries),
        })
    }
}

/// Prepared scope state shared by every clone of an application runtime.
#[derive(Clone)]
pub(crate) struct RuntimeScopePlan {
    topology: Arc<PreparedScopeTopology>,
    orders: Arc<HashMap<ScopeId, Vec<ComponentDescriptor>>>,
    layouts: Arc<ScopeLayouts>,
    seed_destinations: Arc<HashMap<TypeId, SeedDestination>>,
}

impl RuntimeScopePlan {
    pub(crate) fn new(
        topology: Arc<PreparedScopeTopology>,
        orders: Arc<HashMap<ScopeId, Vec<ComponentDescriptor>>>,
        layouts: Arc<ScopeLayouts>,
        seed_destinations: Arc<HashMap<TypeId, SeedDestination>>,
    ) -> Self {
        Self {
            topology,
            orders,
            layouts,
            seed_destinations,
        }
    }

    /// The construction order of each protocol-owned scope, shared with any candidate that
    /// reuses this plan.
    pub(crate) fn orders(&self) -> &Arc<HashMap<ScopeId, Vec<ComponentDescriptor>>> {
        &self.orders
    }

    /// The slot layout of each protocol-owned scope, shared with any candidate that reuses
    /// this plan.
    pub(crate) fn layouts(&self) -> &Arc<ScopeLayouts> {
        &self.layouts
    }

    /// The declared destination of each factory-less scoped component, shared with any
    /// candidate that reuses this plan.
    pub(crate) fn seed_destinations(&self) -> &Arc<HashMap<TypeId, SeedDestination>> {
        &self.seed_destinations
    }
}

impl AppRuntime {
    pub(crate) fn new(
        name: Arc<str>,
        generation: PreparedRuntimeGeneration,
        reloader: ConfigReloader,
        entries: Arc<EntryTable>,
    ) -> Self {
        Self {
            name,
            transitions: RuntimeTransitionCoordinator::new(generation),
            reloader,
            entries,
        }
    }

    /// A non-owning reference to this runtime.
    pub(crate) fn downgrade(&self) -> WeakAppRuntime {
        WeakAppRuntime {
            name: Arc::clone(&self.name),
            transitions: self.transitions.downgrade(),
            reloader: self.reloader.clone(),
            entries: Arc::clone(&self.entries),
        }
    }

    /// The number of committed config reloads (the current config generation).
    pub fn config_generation(&self) -> u64 {
        self.reloader.generation()
    }

    /// The application name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The root (singleton) scope container.
    pub fn root(&self) -> Arc<ScopeContainer> {
        self.view().root().clone()
    }

    /// The validated protocol-owned scope topology used for opening boundaries.
    pub fn scope_topology(&self) -> Arc<PreparedScopeTopology> {
        Arc::clone(&self.view().scope_plan().topology)
    }

    /// The hook manager of the currently committed runtime generation, for running
    /// lifecycle/event hooks by kind.
    ///
    /// Cloned from a pinned current view, so the handle is cheap and stays associated
    /// with the generation that was current when it was taken.
    ///
    /// The manager is generation-bound: after
    /// [`reload_config`](Self::reload_config) publishes a new generation, refetch this
    /// method to run the new generation's hooks. A handle can invoke only while its
    /// original generation's root is still pinned or owned — by a held [`RuntimeView`],
    /// the runtime's current generation, or another root/scope/component owner. A
    /// standalone old handle whose root has since been released returns
    /// [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) outcomes
    /// instead of invoking: hold a [`RuntimeView`] across invocations that must pin one
    /// generation, and refetch this method for current hooks. Components that inject
    /// [`HookManager`] are manager consumers, so the reload invalidation policy
    /// reconstructs them instead of letting them retain a stale manager.
    pub fn hooks(&self) -> HookManager {
        self.view().hooks().clone()
    }

    /// The resolved component set (the effective per-type descriptors). A protocol may
    /// introspect it — the RPC protocol uses it to decide whether the peer is depended
    /// on and therefore worth seeding.
    pub fn resolved_components(&self) -> Arc<[ComponentDescriptor]> {
        Arc::clone(self.view().resolved_components())
    }

    /// Pins the complete current runtime generation for consistent multi-field reads.
    ///
    /// Separate convenience reads may straddle a runtime publication. Hold this view when the
    /// root, component descriptors, effective graph metadata, scope plans, condition state, and
    /// hook manager belong to one generation. It does not freeze separately held legacy live
    /// `Cfg<T>` cells: each slot commits atomically, but no atomicity across handles is promised.
    pub fn view(&self) -> RuntimeView {
        self.transitions.current()
    }

    /// The stable identity of the currently committed runtime generation.
    pub fn generation(&self) -> RuntimeGenerationId {
        self.view().id()
    }

    /// The runtime generation an open scope runs on: the one it opened under or, for a scope
    /// opened under another, its parent's. `None` for a scope this runtime did not open.
    pub fn scope_generation(&self, scope: &ScopeContainer) -> Option<RuntimeGenerationId> {
        let generation = scope.generation_lease::<RuntimeGeneration>()?;

        self.transitions
            .owns(&generation)
            .then(|| RuntimeView::from_generation(generation).id())
    }

    /// How many runtime generations committed after the one an open scope runs on.
    ///
    /// A scope keeps its generation for its whole life, and a scope opened under it inherits
    /// that generation, so a call on an old connection runs on the connection's generation.
    /// A protocol reads this for a
    /// [`Durable`](upwell_core::ScopeGenerations::Durable) scope to drain or reconnect it
    /// once it has fallen behind. `None` for a scope this runtime did not open.
    pub fn generations_behind(&self, scope: &ScopeContainer) -> Option<u64> {
        let opened = self.scope_generation(scope)?;

        Some(self.generation().get().saturating_sub(opened.get()))
    }

    /// The generations that committed transitions displaced but whose roots are still
    /// alive, oldest first.
    ///
    /// A displaced generation stays listed while any pinned [`RuntimeView`], open scope,
    /// or other owner still holds its root. When the last owner releases it, the root's
    /// instances drop, the generation leaves this list, and a `displaced runtime generation
    /// released` event is emitted on `upwell::transition`. The current generation is
    /// never listed.
    pub fn displaced_generations(&self) -> Vec<DisplacedGeneration> {
        self.transitions.displaced_generations()
    }

    pub(crate) async fn begin_transition(&self) -> generation::RuntimeTransition {
        self.transitions.begin().await
    }
}

#[cfg(test)]
mod tests;
