use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::descriptors::{ComponentDescriptor, ProviderDescriptor, ProviderOrderDirection};
use crate::error::Error;

pub(crate) mod cycle;

pub(super) fn build(
    components: &[ComponentDescriptor],
    providers: &[ProviderDescriptor],
) -> crate::Result<HashMap<TypeId, HashMap<TypeId, usize>>> {
    super::selection::validate_provider_components(components, providers)?;

    let components_by_type: HashMap<_, _> = components
        .iter()
        .map(|component| (component.ty.type_id, component))
        .collect();
    let component_ids: HashSet<_> = components.iter().map(|c| c.ty.type_id).collect();
    let mut by_concrete: HashMap<TypeId, Vec<&ProviderDescriptor>> = HashMap::new();
    let mut by_trait: HashMap<TypeId, Vec<&ProviderDescriptor>> = HashMap::new();

    for provider in providers {
        by_concrete
            .entry(provider.concrete_ty.type_id)
            .or_default()
            .push(provider);
        by_trait
            .entry(provider.trait_ty.type_id)
            .or_default()
            .push(provider);
    }

    for source_providers in by_concrete.values() {
        let source = source_providers[0];

        for ordering in source.ordering {
            let target_id = ordering.target.type_id;

            if !component_ids.contains(&target_id) {
                return Err(Error::MissingProviderOrderTarget {
                    component: (source.concrete_ty.type_name)().to_string(),
                    component_id: components_by_type[&source.concrete_ty.type_id]
                        .id
                        .to_string(),
                    target: (ordering.target.type_name)().to_string(),
                    target_type: (ordering.target.type_name)().to_string(),
                });
            }

            if target_id == source.concrete_ty.type_id {
                return Err(Error::SelfProviderOrder {
                    component: (source.concrete_ty.type_name)().to_string(),
                    component_id: components_by_type[&source.concrete_ty.type_id]
                        .id
                        .to_string(),
                    component_type: (source.concrete_ty.type_name)().to_string(),
                });
            }

            for trait_ty in ordering.traits {
                let trait_id = trait_ty.type_id;

                if !source_providers
                    .iter()
                    .any(|p| p.trait_ty.type_id == trait_id)
                {
                    return Err(Error::ProviderOrderSourceTraitMismatch(Box::new(
                        crate::error::ProviderOrderSourceTraitMismatch {
                            component: (source.concrete_ty.type_name)().to_string(),
                            component_id: components_by_type[&source.concrete_ty.type_id]
                                .id
                                .to_string(),
                            component_type: (source.concrete_ty.type_name)().to_string(),
                            trait_name: (trait_ty.type_name)().to_string(),
                            trait_type: (trait_ty.type_name)().to_string(),
                        },
                    )));
                }
            }
        }
    }

    let mut plan = HashMap::new();

    for (trait_id, trait_providers) in by_trait {
        let mut edges: HashMap<TypeId, HashSet<TypeId>> = HashMap::new();
        let mut indegree: HashMap<TypeId, usize> = trait_providers
            .iter()
            .map(|p| (p.concrete_ty.type_id, 0))
            .collect();

        for source in &trait_providers {
            let source_id = source.concrete_ty.type_id;

            for ordering in source.ordering {
                if !ordering.traits.is_empty()
                    && !ordering.traits.iter().any(|ty| ty.type_id == trait_id)
                {
                    continue;
                }

                let target_id = ordering.target.type_id;

                if !trait_providers
                    .iter()
                    .any(|p| p.concrete_ty.type_id == target_id)
                {
                    if ordering.traits.is_empty() {
                        continue;
                    }

                    return Err(Error::ProviderOrderTargetTraitMismatch(Box::new(
                        crate::error::ProviderOrderTargetTraitMismatch {
                            component: (source.concrete_ty.type_name)().to_string(),
                            component_id: components_by_type[&source.concrete_ty.type_id]
                                .id
                                .to_string(),
                            component_type: (source.concrete_ty.type_name)().to_string(),
                            target: (ordering.target.type_name)().to_string(),
                            target_id: components_by_type[&target_id].id.to_string(),
                            target_type: (ordering.target.type_name)().to_string(),
                            trait_name: (source.trait_ty.type_name)().to_string(),
                            trait_type: (source.trait_ty.type_name)().to_string(),
                        },
                    )));
                }

                let (from, to) = match ordering.direction {
                    ProviderOrderDirection::Before => (source_id, target_id),
                    ProviderOrderDirection::After => (target_id, source_id),
                };

                if edges.entry(from).or_default().insert(to) {
                    *indegree.entry(to).or_default() += 1;
                }
            }
        }

        let mut ordered = Vec::with_capacity(trait_providers.len());

        while ordered.len() < trait_providers.len() {
            let next = trait_providers
                .iter()
                .filter(|provider| {
                    let id = provider.concrete_ty.type_id;

                    indegree.get(&id) == Some(&0) && !ordered.contains(&id)
                })
                .min_by(|left, right| {
                    left.priority
                        .cmp(&right.priority)
                        .then_with(|| {
                            (left.concrete_ty.type_name)().cmp((right.concrete_ty.type_name)())
                        })
                        .then_with(|| left.qualifier.cmp(right.qualifier))
                });
            let Some(next) = next else {
                let remaining = trait_providers
                    .iter()
                    .filter(|provider| !ordered.contains(&provider.concrete_ty.type_id))
                    .map(|provider| provider.concrete_ty.type_id)
                    .collect::<Vec<_>>();
                let stable_keys = remaining
                    .iter()
                    .map(|type_id| (*type_id, components_by_type[type_id].id.to_string()))
                    .collect::<HashMap<_, _>>();
                let cyclic = cycle::members(&remaining, &edges, &stable_keys);
                let components = cyclic
                    .iter()
                    .map(|type_id| (components_by_type[type_id].ty.type_name)())
                    .collect::<Vec<_>>()
                    .join(", ");

                return Err(Error::ProviderOrderCycle(Box::new(
                    crate::error::ProviderOrderCycle {
                        trait_name: (trait_providers[0].trait_ty.type_name)().to_string(),
                        trait_type: (trait_providers[0].trait_ty.type_name)().to_string(),
                        components,
                        component_ids: cyclic
                            .iter()
                            .map(|type_id| components_by_type[type_id].id.to_string())
                            .collect(),
                        component_types: cyclic
                            .iter()
                            .map(|type_id| (components_by_type[type_id].ty.type_name)().to_string())
                            .collect(),
                    },
                )));
            };
            let next_id = next.concrete_ty.type_id;

            ordered.push(next_id);

            if let Some(successors) = edges.get(&next_id) {
                for successor in successors {
                    *indegree
                        .get_mut(successor)
                        .expect("provider indegree exists") -= 1;
                }
            }
        }

        plan.insert(
            trait_id,
            ordered
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect(),
        );
    }

    Ok(plan)
}

#[cfg(test)]
mod tests;
