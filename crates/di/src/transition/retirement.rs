//! Consumer-first retirement ordering over an active effective graph.

use std::collections::BTreeSet;

use super::EffectiveGraph;
use super::plan::is_live;

impl EffectiveGraph {
    /// Orders `components` for retirement: every component retires before the
    /// components it holds a fixed dependency on.
    ///
    /// Components linked only through live dependencies may retire in either order, and
    /// ties keep stable component-ID order. A fixed dependency cycle among `components`
    /// cannot be ordered, so its members follow in ID order.
    pub fn retirement_order(&self, components: &BTreeSet<&'static str>) -> Vec<&'static str> {
        let mut remaining = components.clone();
        let mut order = Vec::with_capacity(remaining.len());

        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .filter(|component| {
                    !remaining.iter().any(|consumer| {
                        consumer != *component && self.fixed_targets(consumer).contains(*component)
                    })
                })
                .copied()
                .collect::<Vec<_>>();

            if ready.is_empty() {
                order.extend(remaining.iter().copied());
                break;
            }

            for component in ready {
                remaining.remove(component);
                order.push(component);
            }
        }

        order
    }

    fn fixed_targets(&self, consumer: &str) -> BTreeSet<&'static str> {
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
