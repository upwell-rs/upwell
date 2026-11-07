//! Reading an entry's injected targets from the sources its plan pre-resolved for them.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::Cardinality;

use super::ScopeContainer;
use crate::construct::FromContainer;
use crate::descriptors::Injectable;
use crate::layout::{DepRef, EntryParam, EntryPlan};

impl ScopeContainer {
    /// Extracts `H` as [`extract`](Self::extract) does, first reading it from the source
    /// `plan` pre-resolved for it when `H` is one of the plan's targets and is built.
    ///
    /// `plan` is the plan this scope was opened for, or, on the scope a deferred instance
    /// opens under, the plan it will open with. A target the plan holds no parameter for, or
    /// one not built yet, is extracted as [`extract`](Self::extract) extracts it. A plan made
    /// for another scope is only read for sources outside its own scope.
    ///
    /// # Errors
    ///
    /// As [`extract`](Self::extract).
    pub async fn extract_planned<H: FromContainer>(
        self: &Arc<Self>,
        plan: Option<&EntryPlan>,
    ) -> crate::Result<H> {
        if let Some(value) = plan.and_then(|plan| H::read_planned(self, plan)) {
            return Ok(value);
        }

        self.extract::<H>().await
    }

    /// The single target `H::Target` read from the source `plan` pre-resolved for it:
    /// `Some(None)` for an optional target nothing provides, `None` when the plan holds no
    /// parameter for it, a scope on this chain shadows its source, or the source is not
    /// readable or built here.
    pub(crate) fn planned_one<H: Injectable>(&self, plan: &EntryPlan) -> Option<Option<H>> {
        let param = self.planned_param(plan, TypeId::of::<H::Target>(), Cardinality::One)?;

        match &param.source {
            DepRef::Absent => Some(None),
            source => self.read_param::<H>(plan, source).map(Some),
        }
    }

    /// Every provider of trait `H::Target` a collection target gathers, read from the sources
    /// `plan` pre-resolved for it, under the conditions of [`planned_one`](Self::planned_one).
    pub(crate) fn planned_collection<H: Injectable>(&self, plan: &EntryPlan) -> Option<Vec<H>> {
        let param = self.planned_param(plan, TypeId::of::<H::Target>(), Cardinality::Collection)?;
        let DepRef::Many(provided) = &param.source else {
            return None;
        };
        let mut all = Vec::with_capacity(provided.len());

        for entry in provided.iter() {
            all.push(self.read_param::<H>(plan, &entry.source)?);
        }

        Some(all)
    }

    /// The provider winning each qualifier of trait `H::Target` a keyed target gathers, read
    /// as [`planned_collection`](Self::planned_collection) reads a collection.
    pub(crate) fn planned_keyed<H: Injectable>(
        &self,
        plan: &EntryPlan,
    ) -> Option<HashMap<String, H>> {
        let param = self.planned_param(plan, TypeId::of::<H::Target>(), Cardinality::Keyed)?;
        let DepRef::Many(provided) = &param.source else {
            return None;
        };
        let mut keyed = HashMap::with_capacity(provided.len());

        for entry in provided.iter() {
            let handle = self.read_param::<H>(plan, &entry.source)?;

            keyed.insert(entry.qualifier.to_string(), handle);
        }

        Some(keyed)
    }

    /// The unqualified parameter of `plan` for `target` injected with `cardinality`, unless a
    /// scope on this chain shadows its source.
    fn planned_param<'plan>(
        &self,
        plan: &'plan EntryPlan,
        target: TypeId,
        cardinality: Cardinality,
    ) -> Option<&'plan EntryParam> {
        let param = plan.param(target, None, cardinality)?;

        if param
            .shadowed_by
            .iter()
            .any(|scope| self.can_access(*scope))
        {
            return None;
        }

        Some(param)
    }

    /// The handle a parameter's `source` resolves to from this scope. A slot of the plan's
    /// own scope is read only when this is that scope, from the layout it opened with.
    fn read_param<H: Injectable>(&self, plan: &EntryPlan, source: &DepRef) -> Option<H> {
        match source {
            DepRef::Local(at) if self.scope.id() == plan.scope().id() => {
                self.planned::<H>(self.store.slots()?.layout(), *at)
            }
            DepRef::Local(_) => None,
            source => self.read_enclosing::<H>(source),
        }
    }
}
