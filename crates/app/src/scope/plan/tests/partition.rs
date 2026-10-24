//! Partitioning descriptors into root and scoped build orders, and topology-aware
//! validation.

use super::*;

#[test]
fn factoryless_descriptor_in_undeclared_scope_is_rejected() {
    let descriptors = [descriptor::<MissingSeed>(
        "missing_seed",
        "MissingSeed",
        &MISSING,
        no_factory,
    )];

    let error = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect_err("factory-less descriptors require declared scopes");

    assert!(matches!(
        error,
        Error::UndeclaredScope {
            component,
            scope: MISSING_ID,
        } if component.ends_with("MissingSeed")
    ));
}

#[test]
fn partition_preserves_independent_singleton_declaration_order() {
    let descriptors = [
        descriptor::<SecondSingleton>("z-second", "Second", &Singleton, empty_factory),
        descriptor::<FirstSingleton>("a-first", "First", &Singleton, empty_factory),
    ];
    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.singletons[0].id, descriptors[0].id);
    assert_eq!(plan.singletons[1].id, descriptors[1].id);
}

#[test]
fn partition_preserves_independent_scoped_declaration_order() {
    let descriptors = [
        descriptor::<SecondScoped>("z-second", "Second", &CHILD, empty_factory),
        descriptor::<FirstScoped>("a-first", "First", &CHILD, empty_factory),
    ];
    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.orders[&CHILD_ID][0].id, descriptors[0].id);
    assert_eq!(plan.orders[&CHILD_ID][1].id, descriptors[1].id);
}

#[test]
fn child_order_accepts_root_ancestor_and_local_factoryless_descriptors() {
    let descriptors = [
        descriptor::<Root>("root", "Root", &Singleton, empty_factory),
        descriptor::<ParentSeed>("parent_seed", "ParentSeed", &PARENT, no_factory),
        descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, parent_factory),
    ];

    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.orders[&CHILD_ID].len(), 1);
    assert_eq!(
        plan.orders[&CHILD_ID][0].ty.type_id,
        TypeId::of::<ChildFactory>()
    );
    assert_eq!(
        plan.seed_destinations[&TypeId::of::<ParentSeed>()].scope,
        PARENT_ID
    );
}

#[test]
fn child_order_does_not_treat_sibling_factoryless_descriptors_as_prebuilt() {
    let descriptors = [
        descriptor::<SiblingSeed>("sibling_seed", "SiblingSeed", &SIBLING, no_factory),
        descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, sibling_factory),
    ];

    let error = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect_err("sibling seed is not reachable while planning child");

    assert!(matches!(
        error,
        Error::Di(upwell_di::Error::DependencyCycle(_))
    ));
}

#[test]
fn topology_aware_registry_validation_rejects_sibling_dependencies() {
    let registry = crate::AppRegistry {
        components: vec![
            descriptor::<SiblingSeed>("sibling_seed", "SiblingSeed", &SIBLING, no_factory),
            descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, sibling_factory),
        ],
        providers: Vec::new(),
        config_bindings: Vec::new(),
        condition_facts: Vec::new(),
    };

    let error = registry
        .validate_with_scope_topology(&topology())
        .expect_err("sibling dependency must be unreachable");

    assert!(matches!(
        error,
        Error::Di(upwell_di::Error::ScopeViolation(_))
    ));
}
