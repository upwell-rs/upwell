//! Incremental re-evaluation over changed facts and catalog identity.

use super::*;

#[test]
fn changed_facts_reuse_unrelated_condition_decisions() {
    let default = component::<DefaultAuthenticator>("default-authenticator", factory);
    let custom = component::<CustomAuthenticator>("custom-authenticator", factory)
        .with_condition(&CUSTOM_ENABLED);
    let registry = ComponentRegistry {
        components: vec![custom, default],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");
    let before_snapshot = snapshot(false, "safe");
    let before = catalog
        .evaluate(&before_snapshot)
        .expect("initial facts evaluate");
    let after_snapshot = snapshot(true, "safe");

    let incremental = catalog
        .evaluate_changed(&before, &after_snapshot)
        .expect("changed facts evaluate");
    let complete = catalog
        .evaluate(&after_snapshot)
        .expect("complete evaluation succeeds");

    assert_eq!(incremental.components, complete.components);
    assert_eq!(incremental.providers, complete.providers);
    assert_eq!(incremental.decisions, complete.decisions);
}

#[test]
fn incremental_evaluation_rejects_a_different_catalog_with_the_same_component_ids() {
    let default = component::<DefaultAuthenticator>("default-authenticator", factory);
    let custom = component::<CustomAuthenticator>("custom-authenticator", factory)
        .with_condition(&CUSTOM_ENABLED);
    let registry = ComponentRegistry {
        components: vec![custom, default],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");
    let previous = catalog
        .evaluate(&snapshot(false, "safe"))
        .expect("initial facts evaluate");
    let changed_facts = [facts()[0]];
    let changed_catalog =
        ConditionCatalog::new(&registry, changed_facts).expect("catalog validates");

    assert!(matches!(
        changed_catalog.evaluate_changed(
            &previous,
            &ConditionFactSnapshot::new([(ENABLED, ConditionScalar::Bool(true))])
                .expect("snapshot validates")
        ),
        Err(ConditionError::EvaluationCatalogMismatch)
    ));
}
