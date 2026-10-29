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
use upwell_core::{RuntimeGenerationId, ScopeId, ScopeKey};
use upwell_di::{BoxedComponent, ComponentDescriptor, ScopeContainer};
use upwell_hooks::HookManager;

use crate::scope::{PreparedScopeTopology, ScopeLayouts, ScopeParent, SeedDestination};

mod generation;
mod reentrancy;
mod reload;
mod report;
mod transition_hooks;

pub(crate) use generation::PreparedRuntimeGeneration;
pub use generation::{AppConditionState, DisplacedGeneration, RuntimeView};
use generation::{RuntimeGeneration, RuntimeTransitionCoordinator, WeakTransitionCoordinator};
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
/// [`open_scope`](Self::open_scope).
#[derive(Clone)]
pub struct AppRuntime {
    name: Arc<str>,
    transitions: RuntimeTransitionCoordinator,
    /// The config reloader this runtime's transactional reload drives. Shared with the
    /// app handle, so its serialization lock covers both pipelines.
    reloader: ConfigReloader,
}

/// A non-owning [`AppRuntime`] reference, so framework handles seeded into the runtime's
/// own root never keep that runtime alive.
#[derive(Clone)]
pub(crate) struct WeakAppRuntime {
    name: Arc<str>,
    transitions: WeakTransitionCoordinator,
    reloader: ConfigReloader,
}

impl WeakAppRuntime {
    /// The runtime, while any owner still holds it.
    pub(crate) fn upgrade(&self) -> Option<AppRuntime> {
        let transitions = self.transitions.upgrade()?;

        Some(AppRuntime {
            name: Arc::clone(&self.name),
            transitions,
            reloader: self.reloader.clone(),
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
    ) -> Self {
        Self {
            name,
            transitions: RuntimeTransitionCoordinator::new(generation),
            reloader,
        }
    }

    /// A non-owning reference to this runtime.
    pub(crate) fn downgrade(&self) -> WeakAppRuntime {
        WeakAppRuntime {
            name: Arc::clone(&self.name),
            transitions: self.transitions.downgrade(),
            reloader: self.reloader.clone(),
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

    /// Opens a declared child boundary over one of its declared parents.
    ///
    /// The boundary's prepared scope metadata is used for construction. The caller's
    /// `scope` contributes only its stable identity. Every dynamic seed must have a
    /// factory-less descriptor registered at this exact destination and may appear once.
    pub async fn open_scope(
        &self,
        scope: ScopeKey,
        parent: Arc<ScopeContainer>,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let Some(generation) = parent.generation_lease::<RuntimeGeneration>() else {
            let current = self.view();

            if !parent.belongs_to_registry(current.scopes()) {
                return Err(crate::Error::ForeignScopeParent {
                    child: scope.id(),
                    parent: parent.scope().id(),
                });
            }

            return Err(crate::Error::UnpinnedScopeParent {
                child: scope.id(),
                parent: parent.scope().id(),
            });
        };

        if !self.transitions.owns(&generation) {
            return Err(crate::Error::ForeignScopeParent {
                child: scope.id(),
                parent: parent.scope().id(),
            });
        }

        let view = RuntimeView::from_generation(generation);

        self.open_scope_with_view(view, scope, parent, seeds).await
    }

    /// Opens a root-owned boundary after pinning one current runtime generation.
    ///
    /// Protocols should use this for boundaries whose declared parent is the application root.
    /// Nested boundaries use [`open_scope`](Self::open_scope) so they inherit their parent's
    /// pinned generation.
    pub async fn open_scope_from_root(
        &self,
        scope: ScopeKey,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let view = self.view();
        let root = Arc::clone(view.root());

        self.open_scope_with_view(view, scope, root, seeds).await
    }

    async fn open_scope_with_view(
        &self,
        view: RuntimeView,
        scope: ScopeKey,
        parent: Arc<ScopeContainer>,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let child = scope.id();
        let boundary = view
            .scope_plan()
            .topology
            .boundary(&child)
            .ok_or(crate::Error::UndeclaredScopeOpen { scope: child })?;

        self.validate_parent(&view, child, boundary.parents(), &parent)?;
        self.validate_seeds(&view, child, &seeds)?;

        let layout = view
            .scope_plan()
            .layouts
            .get(&child)
            .ok_or(crate::Error::UndeclaredScopeOpen { scope: child })?;

        ScopeContainer::open_planned(
            parent,
            Arc::clone(view.scopes()),
            layout,
            seeds,
            view.generation_state(),
        )
        .await
        .map_err(crate::Error::from)
    }

    fn validate_parent(
        &self,
        view: &RuntimeView,
        child: ScopeId,
        parents: &'static [ScopeParent],
        parent: &Arc<ScopeContainer>,
    ) -> crate::Result<()> {
        let actual = parent.scope().id();

        if !parent.belongs_to_registry(view.scopes()) {
            return Err(crate::Error::ForeignScopeParent {
                child,
                parent: actual,
            });
        }

        let valid = parents.iter().any(|expected| match expected {
            ScopeParent::Root => Arc::ptr_eq(parent, view.root()),
            ScopeParent::Boundary(expected) => actual == *expected,
        });

        if !valid {
            return Err(crate::Error::InvalidScopeParent {
                child,
                expected: parents.iter().map(ScopeParent::id).collect(),
                actual,
            });
        }

        Ok(())
    }

    fn validate_seeds(
        &self,
        view: &RuntimeView,
        scope: ScopeId,
        seeds: &[BoxedComponent],
    ) -> crate::Result<()> {
        for (index, seed) in seeds.iter().enumerate() {
            let type_id = seed.ty.type_id;
            let type_name = (seed.ty.type_name)();

            if seeds[..index]
                .iter()
                .any(|candidate| candidate.ty.type_id == type_id)
            {
                return Err(crate::Error::DuplicateSeedType { scope, type_name });
            }

            let Some(destination) = view.scope_plan().seed_destinations.get(&type_id) else {
                return Err(crate::Error::UnregisteredSeed { scope, type_name });
            };

            if destination.scope != scope {
                return Err(crate::Error::InvalidSeedDestination {
                    type_name: destination.type_name,
                    expected: destination.scope,
                    actual: scope,
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
