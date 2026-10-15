//! Dependency observation, mapping identity, logical composition and fact snapshots.

use super::*;

#[test]
fn dependency_observation_distinguishes_fixed_and_live_handles() {
    assert_eq!(
        <Arc<ComponentA> as FromContainer>::dependency().observation,
        DependencyObservation::Snapshot
    );
    assert_eq!(
        <Dep<ComponentA> as FromContainer>::dependency().observation,
        DependencyObservation::Live
    );
    assert_eq!(
        <crate::Lazy<Dep<ComponentA>> as FromContainer>::dependency().observation,
        DependencyObservation::Snapshot
    );
    assert_eq!(
        <crate::Deferred<ComponentA> as FromContainer>::dependency().observation,
        DependencyObservation::Snapshot
    );
}

#[test]
fn provider_mapping_identity_is_structural_and_unambiguous() {
    let component = component::<CustomAuthenticator>("custom:auth", None);
    let provider = provider::<CustomAuthenticator>(
        TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
        "qualified:value",
        false,
    );
    let id = provider.mapping_id(&component);

    assert_eq!(id.component, "custom:auth");
    assert_eq!(id.qualifier, "qualified:value");
    assert_eq!(
        id,
        ProviderMappingId::of::<dyn Authenticator>("custom:auth", "qualified:value")
    );
    assert_eq!(
        id.to_string(),
        format!(
            "11#custom:auth{}#{}15#qualified:value",
            id.trait_name().len(),
            id.trait_name()
        )
    );
}

#[test]
fn all_and_any_evaluate_every_child_in_stable_order() {
    static FALSE: ConditionDescriptor = ConditionDescriptor::config_bool("b-false", ENABLED);
    static TRUE: ConditionDescriptor = ConditionDescriptor::config_equals("a-true", MODE, "safe");
    static CHILDREN: [ConditionDescriptor; 2] = [FALSE, TRUE];
    static ROOT: ConditionDescriptor = ConditionDescriptor::all("root", &CHILDREN);
    let registry = ComponentRegistry {
        components: vec![component::<ComponentA>("a", Some(&ROOT))],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let evaluation = catalog
        .evaluate(&snapshot(false, "safe"))
        .expect("facts evaluate");
    let ids = evaluation
        .decisions()
        .iter()
        .map(|decision| decision.condition_id)
        .collect::<Vec<_>>();

    assert_eq!(ids, ["a-true", "b-false", "root"]);
    assert_eq!(evaluation.component_eligible("a"), Some(false));
    assert_eq!(
        catalog.dependencies("a").expect("component is known"),
        [
            ConditionDependency::Config(ENABLED),
            ConditionDependency::Config(MODE),
        ]
    );
}

#[test]
fn empty_all_and_any_have_boolean_identity_values() {
    static EMPTY: [ConditionDescriptor; 0] = [];
    static ALL: ConditionDescriptor = ConditionDescriptor::all("empty-all", &EMPTY);
    static ANY: ConditionDescriptor = ConditionDescriptor::any("empty-any", &EMPTY);
    let all_registry = ComponentRegistry {
        components: vec![component::<ComponentA>("all", Some(&ALL))],
        providers: Vec::new(),
    };
    let any_registry = ComponentRegistry {
        components: vec![component::<ComponentB>("any", Some(&ANY))],
        providers: Vec::new(),
    };

    let all = ConditionCatalog::new(&all_registry, facts())
        .expect("all catalog validates")
        .evaluate(&snapshot(false, "safe"))
        .expect("all evaluates");
    let any = ConditionCatalog::new(&any_registry, facts())
        .expect("any catalog validates")
        .evaluate(&snapshot(false, "safe"))
        .expect("any evaluates");

    assert_eq!(all.component_eligible("all"), Some(true));
    assert_eq!(any.component_eligible("any"), Some(false));
}

#[test]
fn fact_snapshot_rejects_missing_unknown_duplicate_and_wrong_kind_values() {
    let registry = ComponentRegistry {
        components: vec![component::<ComponentA>("a", Some(&CUSTOM_ENABLED))],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    assert!(matches!(
        ConditionFactSnapshot::new([
            (ENABLED, ConditionScalar::Bool(true)),
            (ENABLED, ConditionScalar::Bool(false)),
        ]),
        Err(ConditionError::DuplicateFactValue(ENABLED))
    ));
    assert!(matches!(
        catalog.evaluate(
            &ConditionFactSnapshot::new([(ENABLED, ConditionScalar::Bool(true))])
                .expect("one fact is unique")
        ),
        Err(ConditionError::MissingFactValue(MODE))
    ));
    assert!(matches!(
        catalog.evaluate(
            &ConditionFactSnapshot::new([
                (ENABLED, ConditionScalar::string("true")),
                (MODE, ConditionScalar::string("safe")),
            ])
            .expect("facts are unique")
        ),
        Err(ConditionError::FactKindMismatch { fact: ENABLED, .. })
    ));

    const UNKNOWN: ConfigFactId = ConfigFactId::new("Unknown", "unknown", "unknown");
    assert!(matches!(
        catalog.evaluate(
            &ConditionFactSnapshot::new([
                (ENABLED, ConditionScalar::Bool(true)),
                (MODE, ConditionScalar::string("safe")),
                (UNKNOWN, ConditionScalar::Bool(true)),
            ])
            .expect("facts are unique")
        ),
        Err(ConditionError::UnknownFactValue(UNKNOWN))
    ));
}
