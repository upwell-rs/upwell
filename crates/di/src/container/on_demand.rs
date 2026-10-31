//! Building, when something resolves them, the slots a scope's opening left out.

use std::any::TypeId;
use std::sync::Arc;

use tracing::debug;

use super::ScopeContainer;
use crate::descriptors::component::{ComponentConstructionContext, HandleStorage, Running};
use crate::layout::{DepRef, SlotIdx, SlotLayout};

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

        self.ensure_ancestors(&layout, &needed).await?;
        self.build_missing(&layout, &needed).await
    }

    /// Builds the slots of the scopes around this one that the slots in `needed` read.
    async fn ensure_ancestors(&self, layout: &SlotLayout, needed: &[bool]) -> crate::Result<()> {
        for (index, spec) in layout.slots().iter().enumerate() {
            if !needed[index] {
                continue;
            }

            for dependency in spec.dependencies.iter() {
                if let DepRef::Ancestor { layout, .. } = &dependency.source
                    && let Some(ancestor) = self.parent_of(layout.scope().id())
                {
                    Box::pin(ancestor.ensure_local(dependency.target)).await?;
                }
            }
        }

        Ok(())
    }

    /// Builds the slots in `needed` that are still empty, in layout order, so each reads
    /// dependencies built before it.
    async fn build_missing(
        self: &Arc<Self>,
        layout: &SlotLayout,
        needed: &[bool],
    ) -> crate::Result<()> {
        let _building = self.building.lock().await;
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

            debug!(
                component = %spec.component.name,
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

            let built = factory.build(&mut cx).await?;

            store.insert_at(slot, built)?;
        }

        Ok(())
    }

    /// The container of `scope` among the scopes this one was opened under.
    fn parent_of(&self, scope: upwell_core::ScopeId) -> Option<&Arc<ScopeContainer>> {
        let mut current = self.parent.as_ref();

        while let Some(container) = current {
            if container.scope.id() == scope {
                return Some(container);
            }

            current = container.parent.as_ref();
        }

        None
    }
}
