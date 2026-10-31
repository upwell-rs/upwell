//! Resolving a factory's dependencies through the edges its slot layout planned.

use std::any::TypeId;
use std::sync::{Arc, RwLock};

use upwell_core::Cardinality;

use super::context::BuildStore;
use super::store::SlotStore;
use super::{ComponentConstructionContext, Injectable, ScopeStore, from_boxed};
use crate::layout::{DepRef, PlannedDependency, ProvidedRef, SlotIdx, SlotLayout, TransientPlan};

/// The factory a construction context is running, whose planned edges its resolutions
/// read before falling back to dynamic selection.
#[derive(Default)]
pub(crate) enum Running {
    #[default]
    Unplanned,
    Slot(SlotIdx),
    Transient(Arc<TransientPlan>),
}

/// The outcome of resolving one planned source. `Unavailable` means the planned source
/// cannot answer here, for example an ancestor opened without a layout, so the caller
/// falls back to dynamic resolution.
pub(super) enum Planned<H> {
    Resolved(Option<H>),
    Unavailable,
}

impl<H> Planned<H> {
    fn found(handle: Option<H>) -> Self {
        match handle {
            Some(handle) => Self::Resolved(Some(handle)),
            None => Self::Unavailable,
        }
    }
}

impl ComponentConstructionContext {
    /// Builds this scope into a dense store laid out by `layout`, shared with mid-build
    /// transients only when a factory of the scope captures it.
    pub(crate) fn use_layout(&mut self, layout: Arc<SlotLayout>) {
        let store = ScopeStore::Slots(SlotStore::new(Arc::clone(&layout)));

        self.store = match layout.captures() {
            true => BuildStore::Shared(Arc::new(RwLock::new(store))),
            false => BuildStore::Owned(store),
        };
        self.layout = Some(layout);
    }

    /// Sets the factory whose planned edges the next resolutions use.
    pub(crate) fn set_running(&mut self, running: Running) {
        self.running = running;
    }

    fn planned_dependencies(&self) -> &[PlannedDependency] {
        match &self.running {
            Running::Unplanned => &[],
            Running::Slot(slot) => self
                .layout
                .as_ref()
                .map_or(&[], |layout| &layout.spec(*slot).dependencies),
            Running::Transient(plan) => &plan.dependencies,
        }
    }

    /// The planned source of the running factory's edge for `target`: the edge declared at
    /// `index` when it matches, otherwise the first matching edge by type.
    pub(super) fn planned_edge(
        &self,
        index: Option<usize>,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> Option<&DepRef> {
        let planned = self.planned_dependencies();

        index
            .and_then(|index| PlannedDependency::at(planned, index, target, cardinality))
            .or_else(|| PlannedDependency::find(planned, target, qualifier, cardinality))
    }

    /// Resolves one planned single-value source, building a transient one.
    pub(super) async fn resolve_planned<H: Injectable>(
        &self,
        source: &DepRef,
    ) -> crate::Result<Planned<H>> {
        let planned = match source {
            DepRef::Transient(plan) => Planned::Resolved(self.build_transient::<H>(plan).await?),
            _ => self.read_planned::<H>(source),
        };

        Ok(planned)
    }

    /// Reads one planned single-value source without awaiting. A transient source needs
    /// construction, so it is `Unavailable` here.
    pub(super) fn read_planned<H: Injectable>(&self, source: &DepRef) -> Planned<H> {
        match source {
            DepRef::Absent => Planned::Resolved(None),
            DepRef::Local(at) => Planned::found(
                self.layout
                    .as_ref()
                    .and_then(|layout| self.read_store(|store| store.planned::<H>(layout, *at))),
            ),
            DepRef::Ancestor { layout, at } => Planned::found(
                self.parent
                    .as_ref()
                    .and_then(|parent| parent.ancestor(layout.scope()))
                    .and_then(|ancestor| ancestor.planned::<H>(layout, *at)),
            ),
            DepRef::Root(target) => Planned::found(
                self.parent
                    .as_ref()
                    .and_then(|parent| parent.root_container().root_target::<H>(target)),
            ),
            DepRef::Transient(_) | DepRef::Many(_) | DepRef::Unplanned => Planned::Unavailable,
        }
    }

    /// The planned providers of the running factory's `cardinality` edge for `target`,
    /// with their qualifiers, read at `index` when it matches. `None` when the factory
    /// planned no such edge or one of its providers cannot be answered from the plan.
    pub(super) async fn planned_many<H: Injectable>(
        &self,
        index: Option<usize>,
        target: TypeId,
        cardinality: Cardinality,
    ) -> crate::Result<Option<Vec<(&'static str, H)>>> {
        let Some(provided) = self.planned_providers(index, target, cardinality) else {
            return Ok(None);
        };

        self.resolve_provided::<H>(provided).await
    }

    /// The planned providers of the edge declared at `index`, read without awaiting.
    /// `None` when the edge was not planned or one of its providers needs construction.
    pub(super) fn read_many<H: Injectable>(
        &self,
        index: usize,
        target: TypeId,
        cardinality: Cardinality,
    ) -> Option<Vec<(&'static str, H)>> {
        let provided = self.planned_providers(Some(index), target, cardinality)?;
        let mut read = Vec::with_capacity(provided.len());

        for entry in provided {
            match self.read_planned::<H>(&entry.source) {
                Planned::Resolved(Some(handle)) => read.push((entry.qualifier, handle)),
                Planned::Resolved(None) => {}
                Planned::Unavailable => return None,
            }
        }

        Some(read)
    }

    fn planned_providers(
        &self,
        index: Option<usize>,
        target: TypeId,
        cardinality: Cardinality,
    ) -> Option<&[ProvidedRef]> {
        match self.planned_edge(index, target, None, cardinality)? {
            DepRef::Many(provided) => Some(provided),
            _ => None,
        }
    }

    /// Resolves every provider of a planned collection or keyed source, with its
    /// qualifier. `None` when one of them cannot be answered from the plan.
    async fn resolve_provided<H: Injectable>(
        &self,
        provided: &[ProvidedRef],
    ) -> crate::Result<Option<Vec<(&'static str, H)>>> {
        let mut resolved = Vec::with_capacity(provided.len());

        for entry in provided {
            match self.resolve_planned::<H>(&entry.source).await? {
                Planned::Resolved(Some(handle)) => resolved.push((entry.qualifier, handle)),
                Planned::Resolved(None) => {}
                Planned::Unavailable => return Ok(None),
            }
        }

        Ok(Some(resolved))
    }

    async fn build_transient<H: Injectable>(
        &self,
        plan: &Arc<TransientPlan>,
    ) -> crate::Result<Option<H>> {
        let boxed = crate::container::construct_planned_transient(
            &self.registry,
            self.parent.clone(),
            self.resolver_slot(),
            self.store_handle(),
            Arc::clone(plan),
        )
        .await?;
        let handle = match &plan.provider {
            Some(provider) => from_boxed::<H>(&(provider.erase)(&boxed)),
            None => from_boxed::<H>(&boxed),
        };

        Ok(handle)
    }
}
