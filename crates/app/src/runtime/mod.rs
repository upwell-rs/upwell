//! The protocol-facing runtime handle.
//!
//! [`AppRuntime`] is the cheap-clone handle a [`ProtocolRuntime`](crate::ProtocolRuntime)
//! receives to drive requests through the DI container and reach the app's support
//! systems. It owns the *agnostic* runtime state — the built scope containers, the
//! per-scope construction orders, and the hook manager — that the serve loop used to
//! take as a long argument list, and exposes the scope-opening primitives a protocol
//! drives per connection and per request.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{RuntimeGenerationId, Scope, ScopeId};
use upwell_di::{
    BoxedComponent, ComponentDescriptor, EffectiveGraph, ScopeContainer, ScopeRegistry,
};
use upwell_hooks::HookManager;

use crate::scope::{PreparedScopeTopology, ScopeParent, SeedDestination};

mod generation;

pub use generation::RuntimeView;
use generation::{PreparedRuntimeGeneration, RuntimeGeneration, RuntimeTransitionCoordinator};

/// Everything a protocol needs to drive requests through DI, cheaply cloneable.
///
/// Agnostic to any particular protocol: it holds the built root scope, the per-scope
/// construction orders keyed by stable scope identity, the prepared protocol-owned
/// topology, the resolved component set, and the hook manager. A protocol opens its
/// declared boundaries through [`open_scope`](Self::open_scope).
#[derive(Clone)]
pub struct AppRuntime {
    name: Arc<str>,
    transitions: RuntimeTransitionCoordinator,
    hooks: HookManager,
}

/// Prepared scope state shared by every clone of an application runtime.
#[derive(Clone)]
pub(crate) struct RuntimeScopePlan {
    topology: Arc<PreparedScopeTopology>,
    orders: Arc<HashMap<ScopeId, Vec<ComponentDescriptor>>>,
    seed_destinations: Arc<HashMap<TypeId, SeedDestination>>,
}

impl RuntimeScopePlan {
    pub(crate) fn new(
        topology: Arc<PreparedScopeTopology>,
        orders: Arc<HashMap<ScopeId, Vec<ComponentDescriptor>>>,
        seed_destinations: Arc<HashMap<TypeId, SeedDestination>>,
    ) -> Self {
        Self {
            topology,
            orders,
            seed_destinations,
        }
    }
}

impl AppRuntime {
    pub(crate) fn new(
        name: Arc<str>,
        root: Arc<ScopeContainer>,
        scopes: Arc<ScopeRegistry>,
        scope_plan: RuntimeScopePlan,
        resolved: Arc<[ComponentDescriptor]>,
        graph: EffectiveGraph,
        hooks: HookManager,
    ) -> Self {
        let generation = PreparedRuntimeGeneration::new(root, scopes, scope_plan, resolved, graph);

        Self {
            name,
            transitions: RuntimeTransitionCoordinator::new(generation, hooks.clone()),
            hooks,
        }
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

    /// The hook manager, for running lifecycle/event hooks by kind.
    pub fn hooks(&self) -> &HookManager {
        &self.hooks
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
    /// root, component descriptors, effective graph metadata, and future-scope plans must belong
    /// to one generation. Config and live dependency slots retain their existing per-slot reload
    /// semantics until transactional config integration is added.
    pub fn view(&self) -> RuntimeView {
        self.transitions.current()
    }

    /// The stable identity of the currently committed runtime generation.
    pub fn generation(&self) -> RuntimeGenerationId {
        self.view().id()
    }

    #[allow(
        dead_code,
        reason = "reserved for the next transition-strategy integration"
    )]
    pub(crate) async fn begin_transition(&self) -> generation::RuntimeTransition {
        self.transitions.begin().await
    }

    /// Opens a declared child boundary over its only valid parent.
    ///
    /// The boundary's prepared scope metadata is used for construction. The caller's
    /// `scope` contributes only its stable identity. Every dynamic seed must have a
    /// factory-less descriptor registered at this exact destination and may appear once.
    pub async fn open_scope(
        &self,
        scope: &'static dyn Scope,
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
        scope: &'static dyn Scope,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let view = self.view();
        let root = Arc::clone(view.root());

        self.open_scope_with_view(view, scope, root, seeds).await
    }

    async fn open_scope_with_view(
        &self,
        view: RuntimeView,
        scope: &'static dyn Scope,
        parent: Arc<ScopeContainer>,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let child = scope.id();
        let boundary = view
            .scope_plan()
            .topology
            .boundary(&child)
            .ok_or(crate::Error::UndeclaredScopeOpen { scope: child })?;

        self.validate_parent(&view, child, boundary.parent(), &parent)?;
        self.validate_seeds(&view, child, &seeds)?;

        let order = view
            .scope_plan()
            .orders
            .get(&child)
            .map_or(&[][..], Vec::as_slice);

        ScopeContainer::open_child_with_generation_lease(
            boundary.scope(),
            parent,
            Arc::clone(view.scopes()),
            order,
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
        expected: ScopeParent,
        parent: &Arc<ScopeContainer>,
    ) -> crate::Result<()> {
        let actual = parent.scope().id();

        if !parent.belongs_to_registry(view.scopes()) {
            return Err(crate::Error::ForeignScopeParent {
                child,
                parent: actual,
            });
        }

        let valid = match expected {
            ScopeParent::Root => Arc::ptr_eq(parent, view.root()),
            ScopeParent::Boundary(expected) => actual == expected,
        };

        if !valid {
            return Err(crate::Error::InvalidScopeParent {
                child,
                expected: expected.id(),
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
