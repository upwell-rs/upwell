use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use upwell_core::{Scope, ScopeId, Singleton, Transient};
use upwell_di::{
    ComponentDescriptor, EffectiveGraph, ProviderSelectionModel, SlotLayout, UnplannedConstruction,
    topological_sort,
};

use super::{PreparedScopeTopology, ScopeBoundary};
use crate::entry::EntryDescriptor;

mod layouts;
mod parallel;
mod runs;

pub(crate) use layouts::{EntryPlanning, PlanBase, ScopeLayouts};
pub(crate) use parallel::Parallelism;

/// The validated construction plan for root, transient, and protocol-owned scopes.
#[derive(Debug)]
pub(crate) struct ScopePlan {
    pub(crate) singletons: Vec<ComponentDescriptor>,
    pub(crate) transient: HashMap<TypeId, ComponentDescriptor>,
    pub(crate) orders: HashMap<ScopeId, Vec<ComponentDescriptor>>,
    pub(crate) layouts: ScopeLayouts,
    pub(crate) seed_destinations: HashMap<TypeId, SeedDestination>,
}

/// The declared destination and diagnostic name of a factory-less scoped component.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SeedDestination {
    pub(crate) scope: ScopeId,
    pub(crate) type_name: &'static str,
}

/// Descriptor groups used while computing scope-local construction orders.
struct DescriptorPartitions {
    singletons: Vec<ComponentDescriptor>,
    transient: HashMap<TypeId, ComponentDescriptor>,
    by_scope: HashMap<ScopeId, Vec<ComponentDescriptor>>,
    descriptors_by_scope: HashMap<ScopeId, Vec<ComponentDescriptor>>,
    factoryless_by_scope: HashMap<ScopeId, HashSet<TypeId>>,
    seed_destinations: HashMap<TypeId, SeedDestination>,
}

impl ScopePlan {
    /// Partitions resolved descriptors, computes each boundary's local factory order and
    /// plans `entries` under the `unplanned` construction policy.
    pub(crate) fn partition(
        resolved: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        entries: &[EntryDescriptor],
        unplanned: UnplannedConstruction,
    ) -> crate::Result<Self> {
        let planning = EntryPlanning {
            selection,
            topology,
            unplanned,
            parallelism: Parallelism::available(),
        };

        Self::partition_from(resolved, entries, &planning, None)
    }

    /// Partitions resolved descriptors as [`partition`](Self::partition) does, carrying over
    /// the entry plans of `base` that still hold.
    pub(crate) fn partition_from(
        resolved: &[ComponentDescriptor],
        entries: &[EntryDescriptor],
        planning: &EntryPlanning<'_>,
        base: Option<PlanBase<'_>>,
    ) -> crate::Result<Self> {
        let partitions = DescriptorPartitions::classify(resolved, planning.topology)?;
        let orders = partitions.orders(planning)?;
        let layouts = ScopeLayouts::plan(
            partitions.layouts(&orders, planning)?,
            entries,
            planning,
            base,
        );

        Ok(Self {
            singletons: partitions.singletons,
            transient: partitions.transient,
            orders,
            layouts,
            seed_destinations: partitions.seed_destinations,
        })
    }

    /// The root construction order: `graph`'s singleton order mapped back to these
    /// singletons, reusing the order the effective graph already computed instead of
    /// sorting the singletons a second time.
    pub(crate) fn root_order(&self, graph: &EffectiveGraph) -> Vec<ComponentDescriptor> {
        let by_id = self
            .singletons
            .iter()
            .map(|component| (component.id, *component))
            .collect::<HashMap<_, _>>();

        graph
            .singleton_order()
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect()
    }
}

impl DescriptorPartitions {
    fn classify(
        resolved: &[ComponentDescriptor],
        topology: &PreparedScopeTopology,
    ) -> crate::Result<Self> {
        let mut partitions = Self {
            singletons: Vec::new(),
            transient: HashMap::new(),
            by_scope: HashMap::new(),
            descriptors_by_scope: HashMap::new(),
            factoryless_by_scope: HashMap::new(),
            seed_destinations: HashMap::new(),
        };

        for component in resolved {
            partitions.classify_component(*component, topology)?;
        }

        Ok(partitions)
    }

    fn classify_component(
        &mut self,
        component: ComponentDescriptor,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<()> {
        let scope = component.scope.id();

        if scope == Transient::ID {
            self.transient.insert(component.ty.type_id, component);

            return Ok(());
        }

        if scope == Singleton::ID {
            self.singletons.push(component);

            return Ok(());
        }

        if !topology.contains(&scope) {
            return Err(crate::Error::UndeclaredScope {
                component: (component.ty.type_name)().to_string(),
                scope,
            });
        }

        self.descriptors_by_scope
            .entry(scope)
            .or_default()
            .push(component);

        if component.effective_factory()?.is_some() {
            self.by_scope.entry(scope).or_default().push(component);
        } else {
            self.factoryless_by_scope
                .entry(scope)
                .or_default()
                .insert(component.ty.type_id);
            self.seed_destinations.insert(
                component.ty.type_id,
                SeedDestination {
                    scope,
                    type_name: (component.ty.type_name)(),
                },
            );
        }

        Ok(())
    }

    /// Sorts each boundary's factory-backed components into construction order. Boundaries
    /// are sorted independently, so large topologies sort them in parallel.
    fn orders(
        &self,
        planning: &EntryPlanning<'_>,
    ) -> crate::Result<HashMap<ScopeId, Vec<ComponentDescriptor>>> {
        let topology = planning.topology;
        let root = self
            .singletons
            .iter()
            .map(|component| component.ty.type_id)
            .collect();
        let boundaries = topology.boundaries();
        let work = self.by_scope.values().map(Vec::len).sum();
        let sorted = planning.parallelism.map(boundaries, work, |boundary| {
            let scope = boundary.id();
            let prebuilt = self.prebuilt(&scope, &root, topology);
            let local = self.by_scope.get(&scope).map_or(&[][..], Vec::as_slice);
            let order = topological_sort(
                local,
                &prebuilt,
                planning.selection,
                |consumer, dependency| topology.is_reachable(&consumer, &dependency),
            )?
            .into_iter()
            .copied()
            .collect();

            Ok::<_, crate::Error>((scope, order))
        });

        sorted.into_iter().collect()
    }

    /// Plans each boundary's slot layout from its construction order, parents before
    /// children, so every layout can point into the layouts of the scopes around it.
    /// Boundaries of one rank never enclose each other, so each rank plans in parallel
    /// once it holds enough components.
    fn layouts(
        &self,
        orders: &HashMap<ScopeId, Vec<ComponentDescriptor>>,
        planning: &EntryPlanning<'_>,
    ) -> crate::Result<HashMap<ScopeId, Arc<SlotLayout>>> {
        let mut boundaries: Vec<_> = planning.topology.boundaries().iter().collect();
        let mut layouts = HashMap::with_capacity(boundaries.len());

        boundaries.sort_by_key(|boundary| Reverse(boundary.rank()));

        for rank in boundaries.chunk_by(|left, right| left.rank() == right.rank()) {
            let work = rank
                .iter()
                .filter_map(|boundary| orders.get(&boundary.id()))
                .map(Vec::len)
                .sum();
            let planned = planning.parallelism.map(rank, work, |boundary| {
                self.layout(boundary, orders, &layouts, planning)
            });

            for (boundary, layout) in rank.iter().zip(planned) {
                layouts.insert(boundary.id(), Arc::new(layout?));
            }
        }

        Ok(layouts)
    }

    /// Plans `boundary`'s slot layout against the already planned layouts of its ancestors:
    /// those of `ancestors` that may be open around it, so no edge plans into a sibling.
    fn layout(
        &self,
        boundary: &ScopeBoundary,
        orders: &HashMap<ScopeId, Vec<ComponentDescriptor>>,
        ancestors: &HashMap<ScopeId, Arc<SlotLayout>>,
        planning: &EntryPlanning<'_>,
    ) -> crate::Result<SlotLayout> {
        let scope = boundary.id();
        let seeded: Vec<ComponentDescriptor> = self
            .descriptors_by_scope
            .get(&scope)
            .into_iter()
            .flatten()
            .filter(|component| self.is_seeded(&scope, component))
            .copied()
            .collect();
        let order = orders.get(&scope).map_or(&[][..], Vec::as_slice);
        let enclosing: HashMap<ScopeId, Arc<SlotLayout>> = ancestors
            .iter()
            .filter(|(id, _)| planning.topology.encloses(id, &scope))
            .map(|(id, layout)| (*id, Arc::clone(layout)))
            .collect();
        let layout = SlotLayout::plan(
            boundary.scope(),
            &seeded,
            order,
            &enclosing,
            planning.selection,
            |consumer, dependency| planning.topology.is_reachable(&consumer, &dependency),
        )?;

        Ok(layout)
    }

    fn is_seeded(&self, scope: &ScopeId, component: &ComponentDescriptor) -> bool {
        self.factoryless_by_scope
            .get(scope)
            .is_some_and(|seeded| seeded.contains(&component.ty.type_id))
    }

    fn prebuilt(
        &self,
        scope: &ScopeId,
        root: &HashSet<TypeId>,
        topology: &PreparedScopeTopology,
    ) -> HashSet<TypeId> {
        let mut prebuilt = root.clone();

        for ancestor in topology.common_ancestors(scope).iter().copied() {
            if ancestor == Singleton::ID {
                continue;
            }

            prebuilt.extend(
                self.descriptors_by_scope
                    .get(&ancestor)
                    .into_iter()
                    .flatten()
                    .map(|component| component.ty.type_id),
            );
        }

        prebuilt.extend(
            self.factoryless_by_scope
                .get(scope)
                .into_iter()
                .flatten()
                .copied(),
        );

        prebuilt
    }
}

#[cfg(test)]
mod tests;
