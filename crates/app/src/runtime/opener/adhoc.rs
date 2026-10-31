//! The ad hoc opening API, which validates the boundary, parent and seeds on every call.

use std::sync::Arc;

use upwell_core::{ScopeId, ScopeKey};
use upwell_di::{BoxedComponent, ScopeContainer};

use crate::runtime::{AppRuntime, RuntimeView};
use crate::scope::ScopeParent;

impl AppRuntime {
    /// Opens a declared child boundary over one of its declared parents.
    ///
    /// The boundary's prepared scope metadata is used for construction. The caller's
    /// `scope` contributes only its stable identity. Every dynamic seed must have a
    /// factory-less descriptor registered at this exact destination and may appear once.
    /// A [`ScopeOpener`](super::ScopeOpener) checks all of this once instead of per call.
    pub async fn open_scope(
        &self,
        scope: ScopeKey,
        parent: Arc<ScopeContainer>,
        seeds: Vec<BoxedComponent>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        let view = self.pinned_view(scope.id(), &parent)?;

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

        self.open_in(view, child, parent, seeds).await
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

    pub(super) fn validate_seeds(
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
