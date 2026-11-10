//! Which entries reach each component, so a requirement is checked only against the openers
//! that can open an instance for it.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::EntryKey;
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

use crate::entry::EntryDescriptor;

/// The entries that reach each component placement, by component id, over every dependency
/// edge and trait provider, and the placements every opening may build: the eager components
/// and everything they depend on.
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
    ) -> Self {
        let graph = Graph::of(components, providers);
        let mut reach: HashMap<&'static str, HashSet<&'a EntryKey>> = HashMap::new();
        let eager_roots = components
            .iter()
            .filter(|component| component.construction.is_eager())
            .map(|component| component.ty.type_id);
        let every_opening = graph.closure(eager_roots);

        for entry in entries {
            let targets = (entry.targets)()
                .into_iter()
                .map(|target| target.ty.type_id);

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

/// The components by type and the concrete types providing each trait.
struct Graph<'c> {
    by_type: HashMap<TypeId, Vec<&'c ComponentDescriptor>>,
    providers: HashMap<TypeId, Vec<TypeId>>,
}

impl<'c> Graph<'c> {
    fn of(components: &'c [ComponentDescriptor], providers: &[ProviderDescriptor]) -> Self {
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
        }
    }

    /// The ids of every component placement `roots` reach over dependency edges and trait
    /// providers, the roots' own placements included.
    fn closure(&self, roots: impl Iterator<Item = TypeId>) -> HashSet<&'static str> {
        let mut pending: Vec<TypeId> = roots.collect();
        let mut seen: HashSet<TypeId> = HashSet::new();
        let mut reached: HashSet<&'static str> = HashSet::new();

        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }

            pending.extend(self.providers.get(&ty).into_iter().flatten().copied());

            for component in self.by_type.get(&ty).into_iter().flatten() {
                reached.insert(component.id);
                pending.extend(
                    component
                        .dependencies()
                        .into_iter()
                        .map(|dependency| dependency.ty.type_id),
                );
            }
        }

        reached
    }
}
