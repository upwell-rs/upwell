use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{ScopeId, Singleton, StaticScope, Transient};
use upwell_di::{ComponentDescriptor, EffectiveGraph, ProviderSelectionModel, topological_sort};

use super::PreparedScopeTopology;

/// The validated construction plan for root, transient, and protocol-owned scopes.
#[derive(Debug)]
pub(crate) struct ScopePlan {
    pub(crate) singletons: Vec<ComponentDescriptor>,
    pub(crate) transient: HashMap<TypeId, ComponentDescriptor>,
    pub(crate) orders: HashMap<ScopeId, Vec<ComponentDescriptor>>,
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
    /// Partitions resolved descriptors and computes each boundary's local factory order.
    pub(crate) fn partition(
        resolved: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
    ) -> crate::Result<Self> {
        let partitions = DescriptorPartitions::classify(resolved, topology)?;
        let orders = partitions.orders(topology, selection)?;

        Ok(Self {
            singletons: partitions.singletons,
            transient: partitions.transient,
            orders,
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

    fn prebuilt(
        &self,
        scope: &ScopeId,
        root: &HashSet<TypeId>,
        topology: &PreparedScopeTopology,
    ) -> HashSet<TypeId> {
        let mut prebuilt = root.clone();

        for ancestor in topology.ancestors(scope) {
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
