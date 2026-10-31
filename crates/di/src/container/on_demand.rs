//! Building, when something resolves them, the slots a scope's opening left out.

use std::any::TypeId;
use std::sync::Arc;

use tracing::debug;
use upwell_core::{Cardinality, ResolutionMode, ScopeId};

use super::ScopeContainer;
use crate::descriptors::component::{ComponentConstructionContext, HandleStorage, Running};
use crate::descriptors::{BoxedComponent, ComponentFactoryDescriptor};
use crate::layout::{DepRef, PlannedDependency, SlotIdx, SlotLayout};

impl ScopeContainer {
    /// Builds every slot holding `target` (the component itself or every provider of the
    /// trait) in this scope and the scopes it was opened under that their openings left out,
    /// with the slots those depend on.
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

    /// Builds, where its scope's opening left it out, what resolving `target` reads: for one
    /// value, the component `target` names or the provider runtime selection picks for it and
    /// `qualifier`; for a collection or keyed map, every provider of the trait.
    pub(crate) async fn ensure_for(
        self: &Arc<Self>,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> crate::Result<()> {
        if cardinality != Cardinality::One {
            return self.ensure(target).await;
        }

        let component = self
            .registry
            .selected_runtime_provider(
                target,
                qualifier,
                ResolutionMode::Eager,
                self.scope,
                &|scope| self.can_access(scope),
            )
            .map_or(target, |provider| provider.concrete_ty.type_id);

        self.ensure_component(component).await
    }

    /// Builds the component of type `component` and the slots it reads, where its scope's
    /// opening on this scope's chain left it out.
    pub(crate) async fn ensure_component(self: &Arc<Self>, component: TypeId) -> crate::Result<()> {
        let mut current = Some(self);

        while let Some(container) = current {
            let missing = container.missing(|layout, visit| {
                if let Some(slot) = layout.slot_of(component) {
                    visit(slot);
                }
            });

            if let Some((layout, needed)) = missing {
                return container.build_missing(&layout, &needed).await;
            }

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
        let Some(start) = start else {
            return Ok(());
        };

        for dependency in dependencies {
            match &dependency.source {
                DepRef::Ancestor { layout, at } => {
                    start.ensure_in(layout.scope().id(), at.slot).await?;
                }

                DepRef::Many(provided) => {
                    for entry in provided.iter() {
                        if let DepRef::Ancestor { layout, at } = &entry.source {
                            start.ensure_in(layout.scope().id(), at.slot).await?;
                        }
                    }
                }

                _ => {}
            }
        }

        Ok(())
    }

    /// Builds `slot` of the container of `scope` on this scope's chain, with the slots it
    /// reads, when that container's opening left it out. Checking costs no allocation; only a
    /// build is boxed.
    async fn ensure_in(self: &Arc<Self>, scope: ScopeId, slot: SlotIdx) -> crate::Result<()> {
        let Some(ancestor) = self.in_chain(scope) else {
            return Ok(());
        };
        let Some((layout, needed)) = ancestor.missing(|_, visit| visit(slot)) else {
            return Ok(());
        };

        Box::pin(ancestor.build_missing(&layout, &needed)).await
    }

    async fn ensure_local(self: &Arc<Self>, target: TypeId) -> crate::Result<()> {
        let Some((layout, needed)) =
            self.missing(|layout, visit| layout.for_each_target_slot(target, visit))
        else {
            return Ok(());
        };

        self.build_missing(&layout, &needed).await
    }

    /// The layout of this scope and the slots to build so every slot `slots` visits is built,
    /// or `None` when they all are.
    fn missing(
        &self,
        slots: impl FnOnce(&SlotLayout, &mut dyn FnMut(SlotIdx)),
    ) -> Option<(Arc<SlotLayout>, Vec<bool>)> {
        let store = self.store.slots()?;
        let layout = store.layout();
        let mut needed: Option<Vec<bool>> = None;

        slots(layout, &mut |slot| {
            if layout.spec(slot).factory.is_some() && !store.is_built(slot) {
                let needed = needed.get_or_insert_with(|| vec![false; layout.len()]);

                layout.include(slot, needed);
            }
        });

        Some((Arc::clone(layout), needed?))
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
