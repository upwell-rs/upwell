//! What reaches each component, so a requirement is checked only against the openers that can
//! open an instance for it, and in the scopes a transient component is built in.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{EntryKey, ResolutionMode, ScopeId};
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

use super::OpenerSeeds;
use crate::entry::EntryDescriptor;
use crate::scope::PreparedScopeTopology;

/// What reaches one component placement: the entries whose targets lead to it, and the
/// openers whose roots, such as global middleware, do.
#[derive(Default)]
pub(super) struct ComponentReach<'a> {
    pub(super) entries: HashSet<&'a EntryKey>,
    pub(super) openers: HashSet<&'a str>,
}

/// What reaches each component placement, by component id, the placements every opening may
/// build (the eager components and what they depend on), and the scopes each transient
/// component is resolved from. Every walk follows dependency edges and trait providers to the
/// placement a consumer's scope resolves, never to a placement of the same type in an
/// unrelated scope.
pub(super) struct EntryReach<'a> {
    components: HashMap<&'static str, ComponentReach<'a>>,
    every_opening: HashSet<&'static str>,
    transient_scopes: HashMap<&'static str, HashSet<ScopeId>>,
}

/// The placements one walk reaches, and the scopes it resolves transient ones from.
#[derive(Default)]
struct Walk {
    reached: HashSet<&'static str>,
    transient: Vec<(&'static str, ScopeId)>,
}

impl<'a> EntryReach<'a> {
    /// Walks from every entry's targets, every opener's roots and every eager component, and
    /// finds the scopes every transient component is resolved from.
    pub(super) fn of(
        components: &[ComponentDescriptor],
        providers: &[ProviderDescriptor],
        entries: &'a [EntryDescriptor],
        openers: &'a [OpenerSeeds],
        topology: &PreparedScopeTopology,
    ) -> Self {
        let graph = Graph::of(components, providers, topology);
        let mut reach = Self {
            components: HashMap::new(),
            every_opening: HashSet::new(),
            transient_scopes: HashMap::new(),
        };
        let eager_roots = components
            .iter()
            .filter(|component| component.construction.is_eager())
            .map(|component| (component.ty.type_id, component.scope.id()));
        let every_opening = graph.walk(eager_roots);

        reach.every_opening.extend(&every_opening.reached);
        reach.record_transient(every_opening.transient);

        for entry in entries {
            let scope = entry.scope.id();
            let targets = (entry.targets)()
                .into_iter()
                .map(|target| (target.ty.type_id, scope));
            let walk = graph.walk(targets);

            for id in walk.reached {
                reach
                    .components
                    .entry(id)
                    .or_default()
                    .entries
                    .insert(&entry.key);
            }

            reach.record_transient(walk.transient);
        }

        for opener in openers {
            let scope = opener.scope.id();
            let walk = graph.walk(opener.roots.iter().map(|root| (root.type_id, scope)));

            for id in walk.reached {
                reach
                    .components
                    .entry(id)
                    .or_default()
                    .openers
                    .insert(opener.opener.as_str());
            }

            reach.record_transient(walk.transient);
        }

        reach.record_transient(graph.transient_consumers(components));

        reach
    }

    /// What reaches `component`, or `None` when every opener of its scope must be assumed to
    /// reach it: an eager component and everything it depends on are built on every opening,
    /// and a component nothing reaches may still be resolved by any instance.
    pub(super) fn of_component(
        &self,
        component: &ComponentDescriptor,
    ) -> Option<&ComponentReach<'a>> {
        if self.every_opening.contains(component.id) {
            return None;
        }

        self.components.get(component.id)
    }

    /// The scopes the transient `component` is resolved from, or `None` when no consumer of
    /// it is known.
    pub(super) fn transient_scopes(
        &self,
        component: &ComponentDescriptor,
    ) -> Option<&HashSet<ScopeId>> {
        self.transient_scopes.get(component.id)
    }

    fn record_transient(&mut self, transient: Vec<(&'static str, ScopeId)>) {
        for (id, scope) in transient {
            self.transient_scopes.entry(id).or_default().insert(scope);
        }
    }
}

/// The components by type, the concrete types providing each trait, and the topology that
/// decides which placement a consumer's scope resolves.
struct Graph<'c> {
    by_type: HashMap<TypeId, Vec<&'c ComponentDescriptor>>,
    providers: HashMap<TypeId, Vec<TypeId>>,
    topology: &'c PreparedScopeTopology,
}

/// One type to resolve, from the scope of the component that depends on it, during an
/// invocation of the scope a `Current` edge resolves from.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Step {
    ty: TypeId,
    consumer: ScopeId,
    invocation: ScopeId,
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

    /// Every component placement `roots` reach, each root a type resolved from a scope, over
    /// dependency edges and trait providers, the roots' own placements included. An edge
    /// reaches only the placements its consumer's scope resolves; a `Current` edge resolves
    /// from the root's scope, the invocation the walk starts in.
    fn walk(&self, roots: impl Iterator<Item = (TypeId, ScopeId)>) -> Walk {
        let mut pending: Vec<Step> = roots
            .map(|(ty, scope)| Step {
                ty,
                consumer: scope,
                invocation: scope,
            })
            .collect();
        let mut seen: HashSet<Step> = HashSet::new();
        let mut walk = Walk::default();

        while let Some(step) = pending.pop() {
            if !seen.insert(step) {
                continue;
            }

            let provided = self.providers.get(&step.ty).into_iter().flatten();

            pending.extend(provided.map(|concrete| Step {
                ty: *concrete,
                ..step
            }));

            for component in self.resolved(step.ty, step.consumer) {
                let scope = match component.scope.is_transient() {
                    true => {
                        walk.transient.push((component.id, step.consumer));

                        step.consumer
                    }

                    false => component.scope.id(),
                };

                walk.reached.insert(component.id);
                pending.extend(component.dependencies().into_iter().map(|dependency| {
                    let consumer = match dependency.resolution {
                        ResolutionMode::Current => step.invocation,
                        _ => scope,
                    };

                    Step {
                        ty: dependency.ty.type_id,
                        consumer,
                        invocation: step.invocation,
                    }
                }));
            }
        }

        walk
    }

    /// The scopes every transient component is resolved from by a component that depends on
    /// it, directly or through other transient components.
    fn transient_consumers(
        &self,
        components: &[ComponentDescriptor],
    ) -> Vec<(&'static str, ScopeId)> {
        let roots = components
            .iter()
            .filter(|component| !component.scope.is_transient())
            .flat_map(|component| {
                let scope = component.scope.id();

                component
                    .dependencies()
                    .into_iter()
                    .map(move |dependency| (dependency.ty.type_id, scope))
            })
            .filter(|(ty, _)| self.has_transient_placement(*ty));

        self.walk(roots).transient
    }

    fn has_transient_placement(&self, ty: TypeId) -> bool {
        self.by_type.get(&ty).is_some_and(|placements| {
            placements
                .iter()
                .any(|placement| placement.scope.is_transient())
        })
    }

    /// The placements of `ty` a consumer in `consumer` resolves: its own scope's or an
    /// enclosing one's, or a transient one, built where it is requested.
    fn resolved(
        &self,
        ty: TypeId,
        consumer: ScopeId,
    ) -> impl Iterator<Item = &'c ComponentDescriptor> + '_ {
        self.by_type
            .get(&ty)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |component| {
                component.scope.is_transient()
                    || self.topology.is_reachable(&consumer, &component.scope.id())
            })
    }
}
