//! Building, when something resolves them, the slots a scope's opening left out.

use std::any::TypeId;
use std::sync::Arc;

use tracing::debug;
use upwell_core::ScopeId;

use super::ScopeContainer;
use crate::descriptors::component::{ComponentConstructionContext, HandleStorage, Running};
use crate::descriptors::{BoxedComponent, ComponentFactoryDescriptor};
use crate::layout::{DepRef, PlannedDependency, SlotIdx, SlotLayout};

impl ScopeContainer {
    /// Builds every slot holding `target` in this scope and the scopes it was opened under
    /// that their openings left out, with the slots those depend on.
    ///
    /// Each slot is built once per scope instance, on the runtime generation the instance
    /// opened under, even when several resolutions ask for it at once.
    pub(crate) async fn ensure(self: &Arc<Self>, target: TypeId) -> crate::Result<()> {
        let mut current = Some(self);

        while let Some(container) = current {
            container.ensure_local(target).await?;
            current = container.parent.as_ref();
        }

        Ok(())
    }

    /// Builds the slots of the scopes `start` belongs to, itself included, that
    /// `dependencies` read from an enclosing scope, directly or as one provider of a
    /// collection.
    pub(crate) async fn ensure_ancestor_sources(
        start: Option<&Arc<Self>>,
        dependencies: &[PlannedDependency],
    ) -> crate::Result<()> {
        for dependency in dependencies {
            let ancestors: Vec<ScopeId> = match &dependency.source {
                DepRef::Ancestor { layout, .. } => vec![layout.scope().id()],
                DepRef::Many(provided) => provided
                    .iter()
                    .filter_map(|entry| match &entry.source {
                        DepRef::Ancestor { layout, .. } => Some(layout.scope().id()),
                        _ => None,
                    })
                    .collect(),
                _ => continue,
            };

            for scope in ancestors {
                if let Some(ancestor) = start.and_then(|start| start.in_chain(scope)) {
                    Box::pin(ancestor.ensure_local(dependency.target)).await?;
                }
            }
        }

        Ok(())
    }

    async fn ensure_local(self: &Arc<Self>, target: TypeId) -> crate::Result<()> {
        let Some(store) = self.store.slots() else {
            return Ok(());
        };
        let layout = Arc::clone(store.layout());
        let mut needed: Option<Vec<bool>> = None;

        layout.for_each_target_slot(target, |slot| {
            if layout.spec(slot).factory.is_some() && !store.is_built(slot) {
                let needed = needed.get_or_insert_with(|| vec![false; layout.len()]);

                layout.include(slot, needed);
            }
        });

        let Some(needed) = needed else {
            return Ok(());
        };

        self.build_missing(&layout, &needed).await
    }

    /// Builds the slots in `needed` that are still empty, in layout order, so each reads
    /// dependencies built before it.
    async fn build_missing(
        self: &Arc<Self>,
        layout: &SlotLayout,
        needed: &[bool],
    ) -> crate::Result<()> {
        let store = self
            .store
            .slots()
            .expect("a scope building left-out slots was opened from a layout");

        for (index, spec) in layout.slots().iter().enumerate() {
            let slot = SlotIdx::new(index);

            if !needed[index] || store.is_built(slot) {
                continue;
            }

            let Some(factory) = spec.factory else {
                continue;
            };

            Self::ensure_ancestor_sources(self.parent.as_ref(), &spec.dependencies).await?;
            store
                .build_later(slot, self.build_slot(slot, spec.component.name, factory))
                .await?;
        }

        Ok(())
    }

    /// Runs the factory of `slot` against this open scope.
    async fn build_slot(
        self: &Arc<Self>,
        slot: SlotIdx,
        component: &'static str,
        factory: &ComponentFactoryDescriptor,
    ) -> crate::Result<BoxedComponent> {
        debug!(
            target: "upwell::di",
            component,
            scope = self.scope.name(),
            "constructing component on demand"
        );

        let mut cx = ComponentConstructionContext::new_with_slot(
            self.scope,
            self.parent.clone(),
            Arc::clone(&self.registry),
            self.inherited_resolvers(),
            self.slot.clone(),
        );

        cx.use_opened(Arc::clone(self));
        cx.store_handles(HandleStorage::Fixed);
        cx.set_running(Running::Slot(slot));

        factory.build(&mut cx).await
    }

    /// The container of `scope` among this scope and the scopes it was opened under.
    fn in_chain(self: &Arc<Self>, scope: ScopeId) -> Option<&Arc<ScopeContainer>> {
        let mut current = Some(self);

        while let Some(container) = current {
            if container.scope.id() == scope {
                return Some(container);
            }

            current = container.parent.as_ref();
        }

        None
    }
}
