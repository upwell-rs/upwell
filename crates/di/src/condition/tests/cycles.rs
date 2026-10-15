//! Availability cycles and the scope and manual-registration rules for conditions.

use super::*;

static A_TO_B: ConditionDescriptor = ConditionDescriptor::component_eligible("a-to-b", "b");
static B_TO_A: ConditionDescriptor = ConditionDescriptor::not("b-to-a", &B_TO_A_TARGET);
static B_TO_A_TARGET: ConditionDescriptor =
    ConditionDescriptor::component_eligible("b-to-a-target", "a");

#[test]
fn availability_cycles_are_rejected_with_canonical_edges() {
    let first = ComponentRegistry {
        components: vec![
            component::<ComponentB>("b", Some(&B_TO_A)),
            component::<ComponentA>("a", Some(&A_TO_B)),
        ],
        providers: Vec::new(),
    };
    let second = ComponentRegistry {
        components: first.components.iter().copied().rev().collect(),
        providers: Vec::new(),
    };

    let first_error = match ConditionCatalog::new(&first, facts()) {
        Ok(_) => panic!("cycle validates"),
        Err(error) => error,
    };
    let second_error = match ConditionCatalog::new(&second, facts()) {
        Ok(_) => panic!("cycle validates"),
        Err(error) => error,
    };

    assert_eq!(first_error.to_string(), second_error.to_string());
    assert!(first_error.to_string().contains("a-to-b"));
    assert!(first_error.to_string().contains("b-to-a-target"));
}

#[test]
fn self_cycles_are_rejected() {
    static SELF: ConditionDescriptor = ConditionDescriptor::component_eligible("self", "self");
    let registry = ComponentRegistry {
        components: vec![component::<ComponentA>("self", Some(&SELF))],
        providers: Vec::new(),
    };

    let error = match ConditionCatalog::new(&registry, facts()) {
        Ok(_) => panic!("self-cycle validates"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        ConditionError::AvailabilityCycle(ref edges)
            if edges.len() == 1 && edges[0].from == "self" && edges[0].to == "self"
    ));
}

#[test]
fn manual_and_non_singleton_conditions_are_rejected() {
    let mut manual = ComponentDescriptor::manual(
        "manual",
        "manual",
        TypeDescriptor::of::<ComponentA>("manual"),
        &Singleton,
    );
    manual.condition = Some(&CUSTOM_ENABLED);
    let manual_registry = ComponentRegistry {
        components: vec![manual],
        providers: Vec::new(),
    };
    let mut transient = component::<ComponentB>("transient", Some(&CUSTOM_ENABLED));
    transient.scope = &Transient;
    let transient_registry = ComponentRegistry {
        components: vec![transient],
        providers: Vec::new(),
    };

    assert!(matches!(
        ConditionCatalog::new(&manual_registry, facts()),
        Err(ConditionError::ConditionalManualComponent("manual"))
    ));
    assert!(matches!(
        ConditionCatalog::new(&transient_registry, facts()),
        Err(ConditionError::UnsupportedScope {
            component_id: "transient",
            ..
        })
    ));
}

#[test]
fn manual_registration_cannot_silently_replace_a_conditional_factory() {
    let conditional = component::<ComponentA>("conditional", Some(&CUSTOM_ENABLED));
    let manual = ComponentDescriptor::manual(
        "manual",
        "manual",
        TypeDescriptor::of::<ComponentA>("manual"),
        &Singleton,
    );
    let registry = ComponentRegistry {
        components: vec![conditional, manual],
        providers: Vec::new(),
    };

    assert!(matches!(
        ConditionCatalog::new(&registry, facts()),
        Err(ConditionError::ConditionalManualOverride("conditional"))
    ));
}
