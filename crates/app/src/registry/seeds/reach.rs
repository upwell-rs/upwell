//! Which entries reach each component, so a requirement is checked only against the openers
//! that can open an instance for it.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{EntryKey, ScopeId};
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

use crate::entry::EntryDescriptor;
use crate::scope::PreparedScopeTopology;

/// The entries that reach each component placement, by component id, and the placements
/// every opening may build: the eager components and what they depend on. Both follow
/// dependency edges and trait providers to the placement a consumer's scope resolves, never to
/// a placement of the same type in an unrelated scope.
pub(super) struct EntryReach<'a> {
    entries: HashMap<&'static str, HashSet<&'a EntryKey>>,
    every_opening: HashSet<&'static str>,
}

impl<'a> EntryReach<'a> {
    /// Walks from every entry's targets, and from every eager component, through the
    /// dependencies of the components they reach.
    pub(super) fn of(
        components: &[ComponentDescriptor],
        providers: &[ProviderDescriptor],
        entries: &'a [EntryDescriptor],
        topology: &PreparedScopeTopology,
    ) -> Self {
        let graph = Graph::of(components, providers, topology);
        let mut reach: HashMap<&'static str, HashSet<&'a EntryKey>> = HashMap::new();
        let eager_roots = components
            .iter()
            .filter(|component| component.construction.is_eager())
            .map(|component| (component.ty.type_id, component.scope.id()));
        let every_opening = graph.closure(eager_roots);

        for entry in entries {
            let scope = entry.scope.id();
            let targets = (entry.targets)()
                .into_iter()
                .map(|target| (target.ty.type_id, scope));

            for id in graph.closure(targets) {
                reach.entry(id).or_default().insert(&entry.key);
            }
        }

        Self {
            entries: reach,
            every_opening,
        }
    }

    /// The entries that reach `component`, or `None` when every opener of its scope must be
    /// assumed to reach it: an eager component and everything it depends on are built on
    /// every opening, and a component no entry reaches may still be resolved by any instance.
    pub(super) fn of_component(
        &self,
        component: &ComponentDescriptor,
    ) -> Option<&HashSet<&'a EntryKey>> {
        if self.every_opening.contains(component.id) {
            return None;
        }

        self.entries.get(component.id)
    }
}

/// The components by type, the concrete types providing each trait, and the topology that
/// decides which placement a consumer's scope resolves.
struct Graph<'c> {
    by_type: HashMap<TypeId, Vec<&'c ComponentDescriptor>>,
    providers: HashMap<TypeId, Vec<TypeId>>,
    topology: &'c PreparedScopeTopology,
}

impl<'c> Graph<'c> {
    fn of(
        components: &'c [ComponentDescriptor],
        providers: &[ProviderDescriptor],
        topology: &'c PreparedScopeTopology,
    ) -> Self {
        let mut by_type: HashMap<TypeId, Vec<&ComponentDescriptor>> = HashMap::new();
        let mut provided: HashMap<TypeId, Vec<TypeId>> = HashMap::new();

        for component in components {
            by_type
                .entry(component.ty.type_id)
                .or_default()
                .push(component);
        }

        for provider in providers {
            provided
                .entry(provider.trait_ty.type_id)
                .or_default()
                .push(provider.concrete_ty.type_id);
        }

        Self {
            by_type,
            providers: provided,
            topology,
        }
    }

    /// The ids of every component placement `roots` reach over dependency edges and trait
    /// providers, the roots' own placements included. Each root and edge names a type resolved
    /// from a consumer scope, and reaches only the placements of that type the consumer scope
    /// can resolve: its own scope's, or an enclosing one's.
    fn closure(&self, roots: impl Iterator<Item = (TypeId, ScopeId)>) -> HashSet<&'static str> {
        let mut pending: Vec<(TypeId, ScopeId)> = roots.collect();
        let mut seen: HashSet<(TypeId, ScopeId)> = HashSet::new();
        let mut reached: HashSet<&'static str> = HashSet::new();

        while let Some((ty, consumer)) = pending.pop() {
            if !seen.insert((ty, consumer)) {
                continue;
            }

            let provided = self.providers.get(&ty).into_iter().flatten();

            pending.extend(provided.map(|concrete| (*concrete, consumer)));

            let resolved = self
                .by_type
                .get(&ty)
                .into_iter()
                .flatten()
                .filter(|component| self.resolves(consumer, component));

            for component in resolved {
                let scope = match component.scope.is_transient() {
                    true => consumer,
                    false => component.scope.id(),
                };

                reached.insert(component.id);
                pending.extend(
                    component
                        .dependencies()
                        .into_iter()
                        .map(|dependency| (dependency.ty.type_id, scope)),
                );
            }
        }

        reached
    }

    /// Whether a consumer in `consumer` resolves `component`: a placement of its own scope or an
    /// enclosing one, or a transient one, built where it is requested.
    fn resolves(&self, consumer: ScopeId, component: &ComponentDescriptor) -> bool {
        component.scope.is_transient()
            || self.topology.is_reachable(&consumer, &component.scope.id())
    }
}
