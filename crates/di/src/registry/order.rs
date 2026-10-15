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
mod tests {
    use super::*;
    use crate::descriptors::{BoxedComponent, ProviderOrder};
    use upwell_core::{Singleton, TypeDescriptor};

    trait Trait: Send + Sync {}
    trait OtherTrait: Send + Sync {}
    struct Alpha;
    struct Beta;
    struct Gamma;

    fn erase(_: &BoxedComponent) -> BoxedComponent {
        panic!("ordering tests do not construct providers")
    }

    fn component<T: 'static>(name: &'static str) -> ComponentDescriptor {
        ComponentDescriptor::manual(name, name, TypeDescriptor::of::<T>(name), &Singleton)
    }

    fn provider<T: 'static>(
        name: &'static str,
        qualifier: &'static str,
        ordering: &'static [ProviderOrder],
    ) -> ProviderDescriptor {
        provider_as::<T, dyn Trait>(name, qualifier, ordering)
    }

    fn provider_as<T: 'static, P: ?Sized + 'static>(
        name: &'static str,
        qualifier: &'static str,
        ordering: &'static [ProviderOrder],
    ) -> ProviderDescriptor {
        ProviderDescriptor {
            trait_ty: TypeDescriptor::of::<P>("provider trait"),
            concrete_ty: TypeDescriptor::of::<T>(name),
            qualifier,
            primary: false,
            priority: 0,
            ordering,
            erase,
        }
    }

    #[test]
    fn unconstrained_order_uses_type_name_then_qualifier() {
        let components = [component::<Beta>("Beta"), component::<Alpha>("Alpha")];
        let providers = [
            provider::<Beta>("Beta", "a", &[]),
            provider::<Alpha>("Alpha", "z", &[]),
        ];
        let plan = build(&components, &providers).expect("provider plan");
        let order = &plan[&TypeId::of::<dyn Trait>()];

        assert_eq!(order[&TypeId::of::<Alpha>()], 0);
        assert_eq!(order[&TypeId::of::<Beta>()], 1);
    }

    #[test]
    fn unconstrained_order_uses_priority_before_type_name() {
        let components = [component::<Alpha>("Alpha"), component::<Beta>("Beta")];
        let mut alpha = provider::<Alpha>("Alpha", "alpha", &[]);
        let mut beta = provider::<Beta>("Beta", "beta", &[]);

        alpha.priority = 20;
        beta.priority = -10;

        let plan = build(&components, &[alpha, beta]).expect("provider plan");
        let order = &plan[&TypeId::of::<dyn Trait>()];

        assert_eq!(order[&TypeId::of::<Beta>()], 0);
        assert_eq!(order[&TypeId::of::<Alpha>()], 1);
    }

    #[test]
    fn constraints_form_a_global_topological_order() {
        static BETWEEN: [ProviderOrder; 2] = [
            ProviderOrder {
                target: TypeDescriptor::of::<Alpha>("Alpha"),
                traits: &[],
                direction: ProviderOrderDirection::After,
            },
            ProviderOrder {
                target: TypeDescriptor::of::<Beta>("Beta"),
                traits: &[TypeDescriptor::of::<dyn Trait>("dyn Trait")],
                direction: ProviderOrderDirection::Before,
            },
        ];
        let components = [
            component::<Alpha>("Alpha"),
            component::<Beta>("Beta"),
            component::<Gamma>("Gamma"),
        ];
        let providers = [
            provider::<Beta>("Beta", "beta", &[]),
            provider::<Gamma>("Gamma", "gamma", &BETWEEN),
            provider::<Alpha>("Alpha", "alpha", &[]),
        ];
        let plan = build(&components, &providers).expect("provider plan");
        let order = &plan[&TypeId::of::<dyn Trait>()];

        assert_eq!(order[&TypeId::of::<Alpha>()], 0);
        assert_eq!(order[&TypeId::of::<Gamma>()], 1);
        assert_eq!(order[&TypeId::of::<Beta>()], 2);
    }

    #[test]
    fn priority_orders_providers_that_share_an_after_constraint() {
        static AFTER_ALPHA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Alpha>("Alpha"),
            traits: &[],
            direction: ProviderOrderDirection::After,
        }];
        let components = [
            component::<Alpha>("Alpha"),
            component::<Beta>("Beta"),
            component::<Gamma>("Gamma"),
        ];
        let alpha = provider::<Alpha>("Alpha", "alpha", &[]);
        let mut beta = provider::<Beta>("Beta", "beta", &AFTER_ALPHA);
        let mut gamma = provider::<Gamma>("Gamma", "gamma", &AFTER_ALPHA);

        beta.priority = 20;
        gamma.priority = -20;

        let plan = build(&components, &[beta, alpha, gamma]).expect("provider plan");
        let order = &plan[&TypeId::of::<dyn Trait>()];

        assert_eq!(order[&TypeId::of::<Alpha>()], 0);
        assert_eq!(order[&TypeId::of::<Gamma>()], 1);
        assert_eq!(order[&TypeId::of::<Beta>()], 2);
    }

    #[test]
    fn relative_constraints_take_precedence_over_priority() {
        static BETA_AFTER_ALPHA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Alpha>("Alpha"),
            traits: &[],
            direction: ProviderOrderDirection::After,
        }];
        let components = [component::<Alpha>("Alpha"), component::<Beta>("Beta")];
        let mut alpha = provider::<Alpha>("Alpha", "alpha", &[]);
        let mut beta = provider::<Beta>("Beta", "beta", &BETA_AFTER_ALPHA);

        alpha.priority = 100;
        beta.priority = -100;

        let plan = build(&components, &[beta, alpha]).expect("provider plan");
        let order = &plan[&TypeId::of::<dyn Trait>()];

        assert_eq!(order[&TypeId::of::<Alpha>()], 0);
        assert_eq!(order[&TypeId::of::<Beta>()], 1);
    }

    #[test]
    fn unrestricted_ordering_ignores_traits_not_shared_with_the_target() {
        static ALPHA_AFTER_BETA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Beta>("Beta"),
            traits: &[],
            direction: ProviderOrderDirection::After,
        }];
        let components = [component::<Alpha>("Alpha"), component::<Beta>("Beta")];
        let providers = [
            provider::<Alpha>("Alpha", "alpha", &ALPHA_AFTER_BETA),
            provider_as::<Beta, dyn OtherTrait>("Beta", "beta", &[]),
        ];
        let plan = build(&components, &providers).expect("unshared traits are ignored");

        assert_eq!(plan[&TypeId::of::<dyn Trait>()][&TypeId::of::<Alpha>()], 0);
        assert_eq!(
            plan[&TypeId::of::<dyn OtherTrait>()][&TypeId::of::<Beta>()],
            0
        );
    }

    #[test]
    fn restricted_ordering_requires_the_target_to_provide_the_trait() {
        static ALPHA_AFTER_BETA_AS_TRAIT: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Beta>("Beta"),
            traits: &[TypeDescriptor::of::<dyn Trait>("dyn Trait")],
            direction: ProviderOrderDirection::After,
        }];
        let components = [component::<Alpha>("Alpha"), component::<Beta>("Beta")];
        let providers = [
            provider::<Alpha>("Alpha", "alpha", &ALPHA_AFTER_BETA_AS_TRAIT),
            provider_as::<Beta, dyn OtherTrait>("Beta", "beta", &[]),
        ];

        assert!(matches!(
            build(&components, &providers),
            Err(Error::ProviderOrderTargetTraitMismatch(_))
        ));
    }

    #[test]
    fn orphan_provider_without_ordering_is_rejected_before_ordering() {
        let orphan = provider::<Alpha>("Orphan Alpha", "orphan", &[]);
        let error = build(&[], &[orphan]).expect_err("orphan provider is rejected");
        let Error::ProviderComponentMissing(error) = error else {
            panic!("missing component error is returned before provider ordering");
        };

        assert_eq!(error.trait_type, std::any::type_name::<dyn Trait>());
        assert_eq!(error.component_type, std::any::type_name::<Alpha>());
        assert_eq!(error.component, "Orphan Alpha");
        assert_eq!(error.qualifier, "orphan");
    }

    #[test]
    fn orphan_provider_with_ordering_is_rejected_before_order_metadata() {
        static MISSING_TARGET: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Gamma>("Missing Gamma"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        let orphan = provider::<Alpha>("Orphan Alpha", "ordered", &MISSING_TARGET);

        assert!(matches!(
            build(&[], &[orphan]),
            Err(Error::ProviderComponentMissing(_))
        ));
    }

    #[test]
    fn reports_missing_self_mismatch_and_cycle_errors() {
        static MISSING: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Gamma>("Gamma"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        static SELF: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Alpha>("Alpha"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        static ALPHA_BEFORE_BETA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Beta>("Beta"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        static BETA_BEFORE_ALPHA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Alpha>("Alpha"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        let alpha = component::<Alpha>("Alpha");
        let beta = component::<Beta>("Beta");

        assert!(matches!(
            build(&[alpha], &[provider::<Alpha>("Alpha", "alpha", &MISSING)]),
            Err(Error::MissingProviderOrderTarget { .. })
        ));
        assert!(matches!(
            build(&[alpha], &[provider::<Alpha>("Alpha", "alpha", &SELF)]),
            Err(Error::SelfProviderOrder { .. })
        ));
        assert!(matches!(
            build(
                &[alpha, beta],
                &[
                    provider::<Alpha>("Alpha", "alpha", &ALPHA_BEFORE_BETA),
                    provider::<Beta>("Beta", "beta", &BETA_BEFORE_ALPHA),
                ],
            ),
            Err(Error::ProviderOrderCycle(_))
        ));
    }

    #[test]
    fn cycle_diagnostics_exclude_blocked_downstream_providers() {
        static ALPHA_BEFORE_BETA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Beta>("Beta"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        static BETA_BEFORE_ALPHA_AND_GAMMA: [ProviderOrder; 2] = [
            ProviderOrder {
                target: TypeDescriptor::of::<Alpha>("Alpha"),
                traits: &[],
                direction: ProviderOrderDirection::Before,
            },
            ProviderOrder {
                target: TypeDescriptor::of::<Gamma>("Gamma"),
                traits: &[],
                direction: ProviderOrderDirection::Before,
            },
        ];
        let components = [
            component::<Gamma>("gamma-id"),
            component::<Beta>("beta-id"),
            component::<Alpha>("alpha-id"),
        ];
        let providers = [
            provider::<Gamma>("Gamma", "gamma", &[]),
            provider::<Beta>("Beta", "beta", &BETA_BEFORE_ALPHA_AND_GAMMA),
            provider::<Alpha>("Alpha", "alpha", &ALPHA_BEFORE_BETA),
        ];
        let Error::ProviderOrderCycle(error) =
            build(&components, &providers).expect_err("provider order contains a cycle")
        else {
            panic!("provider cycle error expected");
        };

        assert_eq!(error.component_ids, ["alpha-id", "beta-id"]);
        assert_eq!(
            error.component_types,
            [
                std::any::type_name::<Alpha>(),
                std::any::type_name::<Beta>()
            ]
        );
        assert!(!error.components.contains(std::any::type_name::<Gamma>()));
    }

    #[test]
    fn cycle_diagnostics_are_deterministic_across_registration_permutations() {
        static ALPHA_BEFORE_BETA: [ProviderOrder; 1] = [ProviderOrder {
            target: TypeDescriptor::of::<Beta>("Beta"),
            traits: &[],
            direction: ProviderOrderDirection::Before,
        }];
        static BETA_BEFORE_ALPHA_AND_GAMMA: [ProviderOrder; 2] = [
            ProviderOrder {
                target: TypeDescriptor::of::<Alpha>("Alpha"),
                traits: &[],
                direction: ProviderOrderDirection::Before,
            },
            ProviderOrder {
                target: TypeDescriptor::of::<Gamma>("Gamma"),
                traits: &[],
                direction: ProviderOrderDirection::Before,
            },
        ];
        let alpha = component::<Alpha>("alpha-id");
        let beta = component::<Beta>("beta-id");
        let gamma = component::<Gamma>("gamma-id");
        let alpha_provider = provider::<Alpha>("Alpha", "alpha", &ALPHA_BEFORE_BETA);
        let beta_provider = provider::<Beta>("Beta", "beta", &BETA_BEFORE_ALPHA_AND_GAMMA);
        let gamma_provider = provider::<Gamma>("Gamma", "gamma", &[]);
        let diagnostic = |components: &[ComponentDescriptor], providers: &[ProviderDescriptor]| {
            let Error::ProviderOrderCycle(error) =
                build(components, providers).expect_err("provider order contains a cycle")
            else {
                panic!("provider cycle error expected");
            };

            (error.component_ids, error.component_types, error.components)
        };
        let first = diagnostic(
            &[alpha, beta, gamma],
            &[alpha_provider, beta_provider, gamma_provider],
        );
        let second = diagnostic(
            &[gamma, alpha, beta],
            &[gamma_provider, beta_provider, alpha_provider],
        );

        assert_eq!(first, second);
    }
}
