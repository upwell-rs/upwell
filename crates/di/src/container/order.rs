use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
};

use tracing::{error, trace};
use upwell_core::{ResolutionMode, ScopeId, ScopeKey};

use crate::descriptors::ComponentDescriptor;
use crate::error::Error;
use crate::registry::selection::ProviderSelectionModel;

/// The sortable components keyed by concrete type, built once per sort so wait
/// expansion, tracing, and cycle diagnostics resolve identities without scanning.
type ComponentIndex<'a> = HashMap<TypeId, &'a ComponentDescriptor>;

/// Computes a construction order using the same immutable provider selections as
/// validation and runtime resolution.
///
/// Every `TypeId` in `prebuilt` is already available from seeded instances or a
/// parent scope. `can_reach` describes scope ancestry. Config, dynamic, optional,
/// and non-eager edges impose no construction constraint.
pub fn topological_sort<'a>(
    components: &'a [ComponentDescriptor],
    prebuilt: &HashSet<TypeId>,
    selection: &ProviderSelectionModel,
    can_reach: impl Fn(ScopeId, ScopeId) -> bool,
) -> crate::Result<Vec<&'a ComponentDescriptor>> {
    trace!(total = components.len(), "starting topological sort");

    let index: ComponentIndex<'a> = components
        .iter()
        .map(|component| (component.ty.type_id, component))
        .collect();
    let can_access = |consumer: ScopeKey, dependency: ScopeKey| {
        if dependency.is_transient() {
            return true;
        }

        can_reach(consumer.id(), dependency.id())
    };
    let mut expansion = WaitExpansion {
        selection,
        index: &index,
        can_access: &can_access,
        provider_memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    let waits = components
        .iter()
        .map(|descriptor| {
            (
                descriptor.ty.type_id,
                expansion.component_waits(*descriptor),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut result: Vec<&'a ComponentDescriptor> = Vec::with_capacity(components.len());
    let mut built: HashSet<TypeId> = HashSet::with_capacity(components.len());
    let mut remaining: Vec<&'a ComponentDescriptor> = components.iter().collect();

    while !remaining.is_empty() {
        let before_len = remaining.len();

        remaining.retain(|descriptor| {
            let resolved = waits[&descriptor.ty.type_id]
                .iter()
                .all(|dependency| prebuilt.contains(dependency) || built.contains(dependency));

            if !resolved {
                return true;
            }

            built.insert(descriptor.ty.type_id);
            result.push(descriptor);
            crate::observability::build_position(descriptor, result.len() - 1);

            false
        });

        if remaining.len() == before_len {
            return Err(dependency_cycle(&index, &waits, &remaining));
        }
    }

    trace!(count = result.len(), "topological sort complete");

    Ok(result)
}

/// Reports every cycle among the components the sort could not order and returns
/// the cycle error naming them.
fn dependency_cycle(
    index: &ComponentIndex<'_>,
    waits: &HashMap<TypeId, HashSet<TypeId>>,
    remaining: &[&ComponentDescriptor],
) -> Error {
    let stuck = remaining
        .iter()
        .map(|descriptor| descriptor.name)
        .collect::<Vec<_>>()
        .join(", ");

    for diagnostics in construction_cycle_diagnostics(index, waits, remaining) {
        crate::observability::construction_cycle(
            diagnostics.cycle_id,
            &diagnostics.members,
            &diagnostics.edges,
            &diagnostics.blocked,
        );
    }

    error!(components = %stuck, "dependency cycle detected in component graph");

    Error::DependencyCycle(stuck)
}

/// Transitive wait-set expansion for one scope's construction sort.
struct WaitExpansion<'a> {
    selection: &'a ProviderSelectionModel,
    index: &'a ComponentIndex<'a>,
    can_access: &'a dyn Fn(ScopeKey, ScopeKey) -> bool,
    provider_memo: HashMap<TypeId, HashSet<TypeId>>,
    visiting: HashSet<TypeId>,
}

impl WaitExpansion<'_> {
    fn component_waits(&mut self, descriptor: ComponentDescriptor) -> HashSet<TypeId> {
        let mut waits = HashSet::new();

        for dependency in descriptor.dependencies() {
            let reason = construction_edge_reason(&dependency);
            let accepted = reason == "required-eager";

            crate::observability::construction_edge(&descriptor, &dependency, accepted, reason);

            if accepted {
                self.expand_dependency(descriptor.scope, &dependency, &mut waits);
            }
        }

        crate::observability::construction_waits(
            &descriptor,
            waits
                .iter()
                .filter_map(|wait| self.index.get(wait).copied()),
        );

        waits
    }

    /// The sortable TypeIds a single provider concrete gates: itself when built
    /// in this scope, or the eager dependencies of its transient recipe.
    fn provider_waits(&mut self, concrete: TypeId) -> HashSet<TypeId> {
        if let Some(done) = self.provider_memo.get(&concrete) {
            return done.clone();
        }

        let mut waits = HashSet::new();

        if self.index.contains_key(&concrete) {
            waits.insert(concrete);
        } else if let Some(descriptor) = self
            .selection
            .component(concrete)
            .filter(|descriptor| descriptor.scope.is_transient())
        {
            self.expand_transient(descriptor, &mut waits);
        }

        self.provider_memo.insert(concrete, waits.clone());

        waits
    }

    fn expand_transient(&mut self, descriptor: ComponentDescriptor, waits: &mut HashSet<TypeId>) {
        if !self.visiting.insert(descriptor.ty.type_id) {
            return;
        }

        for dependency in descriptor.dependencies().into_iter().filter(|dependency| {
            !dependency.optional
                && !dependency.dynamic
                && !dependency.config
                && dependency.resolution == ResolutionMode::Eager
        }) {
            self.expand_dependency(descriptor.scope, &dependency, waits);
        }

        self.visiting.remove(&descriptor.ty.type_id);
    }

    fn expand_dependency(
        &mut self,
        consumer: ScopeKey,
        dependency: &upwell_core::DependencyDescriptor,
        waits: &mut HashSet<TypeId>,
    ) {
        if let Some(component) = self.selection.component(dependency.ty.type_id) {
            if (self.can_access)(consumer, component.scope) {
                waits.extend(self.provider_waits(component.ty.type_id));
            } else {
                waits.insert(component.ty.type_id);
            }

            return;
        }

        let providers = self.selection.construction_providers(
            dependency,
            consumer,
            self.can_access,
            &|concrete| self.index.contains_key(&concrete),
        );

        for provider in providers {
            waits.extend(self.provider_waits(provider.concrete_ty.type_id));
        }
    }
}

fn construction_edge_reason(dependency: &upwell_core::DependencyDescriptor) -> &'static str {
    if dependency.optional {
        return "optional";
    }

    if dependency.dynamic {
        return "dynamic";
    }

    if dependency.config {
        return "config-external";
    }

    match dependency.resolution {
        ResolutionMode::Eager => "required-eager",
        ResolutionMode::Lazy => "lazy",
        ResolutionMode::Deferred => "deferred-cycle-break",
        ResolutionMode::Fresh => "fresh",
    }
}

/// One eager construction cycle with its members, internal edges, and the
/// components blocked behind it, all named by stable component ID.
struct CycleDiagnostics<'a> {
    cycle_id: &'a str,
    members: Vec<&'a str>,
    edges: Vec<(&'a str, &'a str)>,
    blocked: Vec<&'a str>,
}

fn construction_cycle_diagnostics<'a>(
    index: &ComponentIndex<'a>,
    waits: &HashMap<TypeId, HashSet<TypeId>>,
    remaining: &[&ComponentDescriptor],
) -> Vec<CycleDiagnostics<'a>> {
    let remaining_ids = remaining
        .iter()
        .map(|component| component.ty.type_id)
        .collect::<Vec<_>>();
    let keys = index
        .iter()
        .map(|(type_id, component)| (*type_id, component.id.to_string()))
        .collect::<HashMap<_, _>>();
    let cycles = crate::registry::order::cycle::components(&remaining_ids, waits, &keys);
    let all_cyclic = cycles.iter().flatten().copied().collect::<HashSet<_>>();

    cycles
        .into_iter()
        .map(|cycle| {
            let cycle_set = cycle.iter().copied().collect::<HashSet<_>>();
            let mut members = cycle
                .iter()
                .map(|type_id| component_id(index, *type_id))
                .collect::<Vec<_>>();
            let mut edges = cycle
                .iter()
                .flat_map(|from| {
                    waits[from]
                        .iter()
                        .filter(|to| cycle_set.contains(to))
                        .map(|to| (component_id(index, *from), component_id(index, *to)))
                })
                .collect::<Vec<_>>();
            let mut blocked = remaining_ids
                .iter()
                .filter(|candidate| !all_cyclic.contains(candidate))
                .filter(|candidate| transitively_waits_on(**candidate, &cycle_set, waits))
                .map(|type_id| component_id(index, *type_id))
                .collect::<Vec<_>>();

            members.sort_unstable();
            edges.sort_unstable();
            blocked.sort_unstable();

            CycleDiagnostics {
                cycle_id: members.first().copied().unwrap_or(""),
                members,
                edges,
                blocked,
            }
        })
        .collect()
}

fn transitively_waits_on(
    component: TypeId,
    targets: &HashSet<TypeId>,
    waits: &HashMap<TypeId, HashSet<TypeId>>,
) -> bool {
    let mut pending = vec![component];
    let mut visited = HashSet::new();

    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }

        for dependency in waits.get(&current).into_iter().flatten() {
            if targets.contains(dependency) {
                return true;
            }

            pending.push(*dependency);
        }
    }

    false
}

fn component_id<'a>(index: &ComponentIndex<'a>, type_id: TypeId) -> &'a str {
    index
        .get(&type_id)
        .map_or("<external>", |component| component.id)
}

#[cfg(test)]
mod tests;
