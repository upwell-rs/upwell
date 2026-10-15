//! Candidate preparation from application condition evaluations and catalog identity.

use super::*;

#[test]
fn app_evaluation_prepares_a_candidate_without_components() {
    let registry = AppRegistry::default();
    let evaluation = registry
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");

    let candidate = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &registry,
        &evaluation,
        &topology(),
    )
    .expect("matching evaluation prepares");

    assert!(candidate.components().is_empty());
    assert!(candidate.singleton_order().is_empty());
}

#[test]
fn non_empty_registry_prepares_through_the_validated_graph_boundary() {
    let mut registry = AppRegistry::default();
    registry
        .components
        .push(upwell_di::root_resolver_descriptor());

    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("non-empty candidate prepares");

    assert_eq!(candidate.components().len(), 1);
    assert!(
        candidate
            .graph()
            .node(upwell_di::ROOT_RESOLVER_ID)
            .is_some()
    );
}

#[test]
fn evaluation_from_another_di_catalog_is_rejected() {
    let source = AppRegistry::default();
    let evaluation = source
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");
    let mut target = AppRegistry::default();
    target
        .components
        .push(upwell_di::root_resolver_descriptor());

    let result = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &target,
        &evaluation,
        &topology(),
    );
    let Err(error) = result else {
        panic!("foreign DI catalog is rejected");
    };

    assert!(matches!(
        error,
        Error::Condition(upwell_di::ConditionError::EvaluationCatalogMismatch)
    ));
}

#[test]
fn evaluation_from_another_app_binding_catalog_is_rejected() {
    let source = AppRegistry::default();
    let evaluation = source
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");
    let mut target = AppRegistry::default();
    target
        .config_bindings
        .push(ConfigBinding::of::<ForeignConfig>("foreign"));

    let result = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &target,
        &evaluation,
        &topology(),
    );
    let Err(error) = result else {
        panic!("foreign application binding catalog is rejected");
    };

    assert!(matches!(
        error,
        Error::ConditionEvaluationApplicationMismatch
    ));
}

#[test]
fn incremental_evaluation_rejects_raw_or_foreign_application_state() {
    let source = AppRegistry::default();
    let previous = source
        .evaluate_conditions(facts(), &snapshot(false))
        .expect("conditions evaluate");
    let mut registry = AppRegistry::default();
    registry
        .config_bindings
        .push(ConfigBinding::of::<ForeignConfig>("foreign"));

    let error = registry
        .evaluate_changed_conditions(facts(), &previous, &snapshot(true))
        .expect_err("incremental evaluation cannot launder application identity");

    assert!(matches!(
        error,
        Error::ConditionEvaluationApplicationMismatch
    ));
}

#[test]
fn incremental_app_evaluation_remains_accepted_by_candidate_preparation() {
    let registry = AppRegistry::default();
    let initial = registry
        .evaluate_conditions(facts(), &snapshot(false))
        .expect("initial conditions evaluate");
    let changed = registry
        .evaluate_changed_conditions(facts(), &initial, &snapshot(true))
        .expect("incremental conditions evaluate");

    CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &registry,
        &changed,
        &topology(),
    )
    .expect("incremental evaluation retains application identity");
}
