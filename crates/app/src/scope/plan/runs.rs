//! Plans of the work `ScopeBuilder::run` runs, made the first time each kind of work runs.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use upwell_core::{DependencyDescriptor, ScopeId};
use upwell_di::{EntryPlan, ProviderSelectionModel, SlotLayout, UnplannedConstruction};

use crate::scope::PreparedScopeTopology;

/// The plan of every kind of work run in a scope of one generation, keyed by the scope and the
/// type of the work's targets.
///
/// Work is planned like an entry, against the generation's layout, the first time it runs, so
/// later runs read the plan without planning. A new generation starts with no plans.
#[derive(Debug, Default)]
pub(crate) struct RunPlans {
    plans: RwLock<HashMap<(ScopeId, TypeId), Arc<EntryPlan>>>,
}

/// What a run is planned with: the layout of its scope, the generation's provider selection and
/// topology, and its construction policy.
pub(crate) struct RunPlanning<'a> {
    pub(crate) layout: &'a SlotLayout,
    pub(crate) selection: &'a ProviderSelectionModel,
    pub(crate) topology: &'a PreparedScopeTopology,
    pub(crate) unplanned: UnplannedConstruction,
}

impl RunPlans {
    /// The plan of work into `scope` whose targets have type `targets`, planned from
    /// `planning` with `descriptors` on the first call.
    pub(crate) fn plan(
        &self,
        scope: ScopeId,
        targets: TypeId,
        descriptors: impl FnOnce() -> Vec<DependencyDescriptor>,
        planning: &RunPlanning<'_>,
    ) -> Arc<EntryPlan> {
        let key = (scope, targets);

        if let Some(plan) = self.read().get(&key) {
            return Arc::clone(plan);
        }

        let plan = Arc::new(planning.layout.plan_entry(
            &descriptors(),
            planning.selection,
            |consumer, dependency| planning.topology.is_reachable(&consumer, &dependency),
            planning.unplanned,
        ));
        let mut plans = self
            .plans
            .write()
            .expect("the run plan lock is never poisoned");

        Arc::clone(plans.entry(key).or_insert(plan))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<(ScopeId, TypeId), Arc<EntryPlan>>> {
        self.plans
            .read()
            .expect("the run plan lock is never poisoned")
    }

    /// The number of plans made so far.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.read().len()
    }
}
