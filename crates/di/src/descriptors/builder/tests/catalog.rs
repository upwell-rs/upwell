//! #296 acceptance: a default provider plus a config-gated custom provider declared with
//! builders only, evaluated identically in every declaration order, with errors that
//! name the offending declaration by stable IDs.

use std::any::TypeId;

use upwell_core::{
    ConditionPredicate, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor,
    DescriptorSource, ResolutionMode, Scope, Singleton,
};

use super::*;
use crate::{ComponentRegistry, ConditionCatalog, ConditionError, ConditionFactSnapshot};

static SWEDISH: ComponentDescriptor =
    ComponentDescriptor::of::<Swedish>().with_factories(factories![swedish]);

static SWEDISH_GREETER: ProviderDescriptor =
    ProviderDescriptor::of::<Swedish, dyn Greeter>("dyn Greeter");

static ENGLISH_GREETER: ProviderDescriptor =
    ProviderDescriptor::of::<English, dyn Greeter>("dyn Greeter").primary();

fn facts() -> [ConfigFactDescriptor; 1] {
    [ConfigFactDescriptor {
        id: ENABLED,
        kind: ConditionScalarKind::Bool,
        source: DescriptorSource::caller(),
    }]
}

fn snapshot(enabled: bool) -> ConditionFactSnapshot {
    ConditionFactSnapshot::new([(ENABLED, ConditionScalar::Bool(enabled))]).expect("one fact value")
}

/// The eligible component IDs and the selected greeter's qualifier for one declaration
/// order and one value of the enabling fact.
fn evaluate(
    components: Vec<ComponentDescriptor>,
    providers: Vec<ProviderDescriptor>,
    enabled: bool,
) -> (Vec<&'static str>, &'static str) {
    let registry = ComponentRegistry {
        components,
        providers,
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("the catalog validates");
    let evaluation = catalog
        .evaluate(&snapshot(enabled))
        .expect("the facts evaluate");
    let eligible = evaluation.eligible_registry();
    let resolved = eligible
        .resolved_components()
        .expect("the eligible components resolve");
    let mut ids = resolved
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let selected = eligible
        .provider_selection_model(&resolved)
        .expect("the eligible providers select")
        .select_runtime_one(
            TypeId::of::<dyn Greeter>(),
            None,
            ResolutionMode::Eager,
            Singleton::KEY,
            &|_, _| true,
        )
        .expect("one greeter is selected")
        .provider
        .qualifier;

    ids.sort_unstable();

    (ids, selected)
}

#[test]
fn the_gated_custom_provider_replaces_the_default_in_every_declaration_order() {
    let orders = [
        (
            vec![SWEDISH, ENGLISH],
            vec![SWEDISH_GREETER, ENGLISH_GREETER],
        ),
        (
            vec![ENGLISH, SWEDISH],
            vec![SWEDISH_GREETER, ENGLISH_GREETER],
        ),
        (
            vec![SWEDISH, ENGLISH],
            vec![ENGLISH_GREETER, SWEDISH_GREETER],
        ),
        (
            vec![ENGLISH, SWEDISH],
            vec![ENGLISH_GREETER, SWEDISH_GREETER],
        ),
    ];

    for (components, providers) in orders {
        assert_eq!(
            evaluate(components.clone(), providers.clone(), false),
            (vec!["swedish"], "swedish"),
            "the default provider serves while the custom one is disabled"
        );
        assert_eq!(
            evaluate(components, providers, true),
            (vec!["english", "swedish"], "english"),
            "the enabled custom provider is primary"
        );
    }
}

#[test]
fn a_duplicate_condition_id_names_its_component_and_condition() {
    static TWICE: ConditionDescriptor =
        ConditionDescriptor::all("greeter-enabled", &[ENABLED_CONDITION]);

    let registry = ComponentRegistry {
        components: vec![SWEDISH, ENGLISH.with_condition(&TWICE)],
        providers: vec![SWEDISH_GREETER, ENGLISH_GREETER],
    };
    let error = ConditionCatalog::new(&registry, facts())
        .err()
        .expect("the repeated condition ID is rejected");

    assert!(matches!(TWICE.predicate, ConditionPredicate::All(_)));
    assert!(
        matches!(
            error,
            ConditionError::DuplicateConditionId {
                component_id: "english",
                condition_id: "greeter-enabled",
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_missing_fact_names_its_component_and_condition() {
    let registry = ComponentRegistry {
        components: vec![SWEDISH, ENGLISH],
        providers: vec![SWEDISH_GREETER, ENGLISH_GREETER],
    };
    let error = ConditionCatalog::new(&registry, [])
        .err()
        .expect("the undeclared fact is rejected");

    assert!(
        matches!(
            error,
            ConditionError::MissingFactReference {
                component_id: "english",
                condition_id: "greeter-enabled",
                fact: ENABLED,
            }
        ),
        "{error:?}"
    );
}
