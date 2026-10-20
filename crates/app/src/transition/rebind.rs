//! Live rebinding of retained singleton consumers.
//!
//! A consumer's `Dep<T>` field shares the live cell of the root that constructed it, and
//! keeps sharing that exact cell for as long as later generations retain the consumer.
//! A retained consumer whose live dependency changed therefore follows the change by
//! republishing *its* held cell with the candidate's value. Every rebind is prepared
//! before commit and applied infallibly inside the terminal publication; a failed or
//! dropped attempt never touches a held cell.

use std::collections::BTreeMap;
use std::sync::Arc;

use upwell_di::{DependencyDemand, DependencyDemandId, LiveCell, LiveRebind, ScopeContainer};

use super::{CandidateGraph, ComponentTransitionStrategy, ResolvedTransitionPlan, RestartReason};
use crate::RuntimeView;

/// The live cells retained singleton consumers hold, keyed by dependency demand.
///
/// A demand absent from the map belongs to a consumer constructed in the generation that
/// owns the map, so its cell is that generation's root slot. A `None` entry records a
/// retained demand whose held cell could not be identified; rebinding it is rejected
/// instead of republishing the wrong cell.
#[derive(Clone, Default)]
pub(crate) struct HeldLiveCells {
    cells: Arc<BTreeMap<DependencyDemandId, Option<LiveCell>>>,
}

/// The prepared republications one commit applies, in no particular order.
#[derive(Default)]
pub(crate) struct LiveRebinds {
    rebinds: Vec<LiveRebind>,
}

impl HeldLiveCells {
    /// The cell `demand`'s consumer holds in the generation owning this map.
    fn held(&self, root: &ScopeContainer, demand: &DependencyDemand) -> Option<LiveCell> {
        match self.cells.get(&demand.id) {
            Some(cell) => cell.clone(),

            None => root.live_cell(*demand.targets.first()?).ok(),
        }
    }
}

impl LiveRebinds {
    /// Publishes every prepared value into its held cell.
    pub(crate) fn commit(self) {
        for rebind in self.rebinds {
            rebind.commit();
        }
    }
}

impl ResolvedTransitionPlan {
    /// Records the live cells every retained consumer holds and prepares the in-place
    /// rebinds the plan requires, reading values from the built candidate root.
    ///
    /// A rebind whose held or candidate cell cannot be identified, or whose cells hold
    /// different types, is rejected with [`RestartReason::LiveRebindUnsupported`]. No
    /// cell is touched here; the returned [`LiveRebinds`] apply at commit.
    pub(crate) fn prepare_live_rebinds(
        &self,
        candidate: &CandidateGraph,
        active: &RuntimeView,
        candidate_root: &ScopeContainer,
    ) -> crate::Result<(HeldLiveCells, LiveRebinds)> {
        let active_held = active.held_live_cells();
        let mut cells = BTreeMap::new();
        let mut rebinds = Vec::new();

        for decision in self.decisions_with(ComponentTransitionStrategy::Retain) {
            let (Some(before), Some(after)) = (
                active.effective_graph().node(decision.component),
                candidate.graph.node(decision.component),
            ) else {
                continue;
            };

            for (before, after) in before.dependencies.iter().zip(&after.dependencies) {
                if !before.rebindable() || !after.rebindable() {
                    continue;
                }

                let held = active_held.held(active.root(), before);

                if self.rebinds_binding(after.id) {
                    let rebind = held
                        .as_ref()
                        .zip(after.targets.first())
                        .and_then(|(held, target)| {
                            held.rebind_to(&candidate_root.live_cell(*target).ok()?)
                        })
                        .ok_or_else(|| {
                            self.restart(decision, RestartReason::LiveRebindUnsupported)
                        })?;

                    rebinds.push(rebind);
                }

                cells.insert(after.id, held);
            }
        }

        let held = HeldLiveCells {
            cells: Arc::new(cells),
        };

        Ok((held, LiveRebinds { rebinds }))
    }

    fn rebinds_binding(&self, demand: DependencyDemandId) -> bool {
        self.structural
            .bindings
            .iter()
            .any(|binding| binding.dependency == demand && binding.in_place)
    }
}
