//! Consumer-first retirement ordering over an active effective graph.

use std::collections::{BTreeMap, BTreeSet};

use super::EffectiveGraph;
use super::plan::is_live;

impl EffectiveGraph {
    /// Orders `components` for retirement: every component retires before the
    /// components it holds a fixed dependency on.
    ///
    /// Components linked only through live dependencies may retire in either order, and
    /// ties keep stable component-ID order. A fixed dependency cycle among `components`
    /// cannot be ordered, so its members follow in ID order.
    ///
    /// Retirement proceeds in layers: each layer is every remaining component no remaining
    /// component holds a fixed dependency on, in ID order. Each component's fixed targets
    /// are resolved once, so ordering is linear in the components and their edges.
    pub fn retirement_order(&self, components: &BTreeSet<&'static str>) -> Vec<&'static str> {
        let targets = components
            .iter()
            .map(|component| {
                let held = self
                    .fixed_targets(component)
                    .into_iter()
                    .filter(|target| target != component && components.contains(target))
                    .collect::<Vec<_>>();

                (*component, held)
            })
            .collect::<BTreeMap<_, _>>();
        let mut consumers = components
            .iter()
            .map(|component| (*component, 0_usize))
            .collect::<BTreeMap<_, _>>();

        for target in targets.values().flatten() {
            if let Some(count) = consumers.get_mut(target) {
                *count += 1;
            }
        }

        let mut order = Vec::with_capacity(components.len());
        let mut layer = consumers
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(component, _)| *component)
            .collect::<Vec<_>>();

        while !layer.is_empty() {
            let mut next = Vec::new();

            for component in &layer {
                for target in &targets[component] {
                    if let Some(count) = consumers.get_mut(target) {
                        *count -= 1;

                        if *count == 0 {
                            next.push(*target);
                        }
                    }
                }
            }

            order.append(&mut layer);
            next.sort_unstable();
            layer = next;
        }

        if order.len() < components.len() {
            let placed = order.iter().copied().collect::<BTreeSet<_>>();

            order.extend(components.difference(&placed).copied());
        }

        order
    }

    pub(super) fn fixed_targets(&self, consumer: &str) -> BTreeSet<&'static str> {
        self.nodes
            .get(consumer)
            .into_iter()
            .flat_map(|node| node.dependencies.iter())
            .filter(|dependency| !is_live(dependency))
            .flat_map(|dependency| dependency.targets.iter())
            .filter_map(|target| target.component_id())
            .collect()
    }
}
