use super::*;
use crate::descriptors::ProviderOrder;
use crate::test_support::planned_provider;
use upwell_core::{Singleton, TypeDescriptor};

trait Trait: Send + Sync {}
trait OtherTrait: Send + Sync {}
struct Alpha;
struct Beta;
struct Gamma;

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
    planned_provider(
        TypeDescriptor::of::<P>("provider trait"),
        TypeDescriptor::of::<T>(name),
        qualifier,
    )
    .ordering(ordering)
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
