use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use upwell_core::{Scope, ScopeId, Singleton, Transient};
use upwell_di::{
    ComponentDescriptor, EffectiveGraph, EntryPlan, ProviderSelectionModel, SlotLayout,
    topological_sort,
};

use super::PreparedScopeTopology;
use crate::entry::{EntryDescriptor, EntryId};

/// The validated construction plan for root, transient, and protocol-owned scopes.
#[derive(Debug)]
pub(crate) struct ScopePlan {
    pub(crate) singletons: Vec<ComponentDescriptor>,
    pub(crate) transient: HashMap<TypeId, ComponentDescriptor>,
    pub(crate) orders: HashMap<ScopeId, Vec<ComponentDescriptor>>,
    pub(crate) layouts: ScopeLayouts,
    pub(crate) seed_destinations: HashMap<TypeId, SeedDestination>,
}

/// The prepared slot layout of every protocol-owned scope, and the plan of every entry
/// opening one of them.
///
/// Entry plans are indexed like the application's [`EntryTable`](crate::EntryTable), whose
/// entries are fixed while each runtime generation plans them against its own layouts.
#[derive(Debug, Default)]
pub(crate) struct ScopeLayouts {
    by_scope: HashMap<ScopeId, Arc<SlotLayout>>,
    entries: Box<[Option<EntryPlan>]>,
}

impl ScopeLayouts {
    /// Plans every entry of `entries` against the scope layouts `by_scope`. An entry into
    /// the root scope has no plan.
    pub(crate) fn new(
        by_scope: HashMap<ScopeId, Arc<SlotLayout>>,
        entries: &[EntryDescriptor],
    ) -> Self {
        let entries = crate::entry::in_table_order(entries)
            .map(|entry| {
                by_scope
                    .get(&entry.scope.id())
                    .map(|layout| layout.plan_entry(&(entry.targets)()))
            })
            .collect();

        Self { by_scope, entries }
    }

    /// The slot layout of `scope`.
    pub(crate) fn get(&self, scope: &ScopeId) -> Option<&Arc<SlotLayout>> {
        self.by_scope.get(scope)
    }

    /// The plan of the entry with dense identity `id`.
    pub(crate) fn entry(&self, id: EntryId) -> Option<&EntryPlan> {
        self.entries.get(id.index())?.as_ref()
    }
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
    /// Partitions resolved descriptors and computes each boundary's local factory order.
    pub(crate) fn partition(
        resolved: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        entries: &[EntryDescriptor],
    ) -> crate::Result<Self> {
        let partitions = DescriptorPartitions::classify(resolved, topology)?;
        let orders = partitions.orders(topology, selection)?;
        let layouts = ScopeLayouts::new(partitions.layouts(&orders, topology, selection)?, entries);

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

    fn orders(
        &self,
        topology: &PreparedScopeTopology,
        selection: &ProviderSelectionModel,
    ) -> crate::Result<HashMap<ScopeId, Vec<ComponentDescriptor>>> {
        let root = self
            .singletons
            .iter()
            .map(|component| component.ty.type_id)
            .collect();
        let mut orders = HashMap::new();

        for boundary in topology.boundaries() {
            let scope = boundary.id();
            let prebuilt = self.prebuilt(&scope, &root, topology);
            let local = self.by_scope.get(&scope).map_or(&[][..], Vec::as_slice);
            let order = topological_sort(local, &prebuilt, selection, |consumer, dependency| {
                topology.is_reachable(&consumer, &dependency)
            })?
            .into_iter()
            .copied()
            .collect();

            orders.insert(scope, order);
        }

        Ok(orders)
    }

    /// Plans each boundary's slot layout from its construction order, parents before
    /// children, so every layout can point into the layouts of the scopes around it.
    fn layouts(
        &self,
        orders: &HashMap<ScopeId, Vec<ComponentDescriptor>>,
        topology: &PreparedScopeTopology,
        selection: &ProviderSelectionModel,
    ) -> crate::Result<HashMap<ScopeId, Arc<SlotLayout>>> {
        let mut boundaries: Vec<_> = topology.boundaries().iter().collect();
        let mut layouts = HashMap::with_capacity(boundaries.len());

        boundaries.sort_by_key(|boundary| Reverse(boundary.rank()));

        for boundary in boundaries {
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
            let layout = SlotLayout::plan(
                boundary.scope(),
                &seeded,
                order,
                &layouts,
                selection,
                |consumer, dependency| topology.is_reachable(&consumer, &dependency),
            )?;

            layouts.insert(scope, Arc::new(layout));
        }

        Ok(layouts)
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
