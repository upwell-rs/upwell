//! Tooling evaluation: pure callbacks and structured predicates decide, trusted callbacks
//! never run, and undecided outcomes propagate with three-valued logic.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

static TRUSTED_CALLS: AtomicUsize = AtomicUsize::new(0);
static PURE_CALLS: AtomicUsize = AtomicUsize::new(0);

fn trusted(_: ConditionContext<'_>) -> bool {
    TRUSTED_CALLS.fetch_add(1, Ordering::SeqCst);

    true
}

fn pure_enabled(context: ConditionContext<'_>) -> bool {
    PURE_CALLS.fetch_add(1, Ordering::SeqCst);

    context
        .fact(ENABLED)
        .and_then(ConditionScalar::as_bool)
        .expect("the declared fact is supplied")
}

static TRUSTED: ConditionCallback = ConditionCallback {
    kind: "test/trusted",
    inputs: &[],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: trusted,
};

static PURE: ConditionCallback = ConditionCallback {
    kind: "test/pure",
    inputs: &[ConditionInputId::Fact(ENABLED)],
    expression: None,
    purity: ConditionPurity::Pure,
    evaluate: pure_enabled,
};

static OPAQUE: ConditionDescriptor = ConditionDescriptor::callback("opaque", &TRUSTED);
static FOLLOWS_A: ConditionDescriptor = ConditionDescriptor::component_eligible("follows-a", "a");
static DISABLED_AND_OPAQUE_CHILDREN: [ConditionDescriptor; 2] = [
    ConditionDescriptor::config_bool("enabled", ENABLED),
    ConditionDescriptor::callback("opaque", &TRUSTED),
];
static DISABLED_AND_OPAQUE: ConditionDescriptor =
    ConditionDescriptor::all("all", &DISABLED_AND_OPAQUE_CHILDREN);
static NOT_ENABLED: ConditionDescriptor = ConditionDescriptor::config_bool("enabled", ENABLED);
static NOT_ENABLED_OR_OPAQUE_CHILDREN: [ConditionDescriptor; 2] = [
    ConditionDescriptor::not("not-enabled", &NOT_ENABLED),
    ConditionDescriptor::callback("opaque", &TRUSTED),
];
static NOT_ENABLED_OR_OPAQUE: ConditionDescriptor =
    ConditionDescriptor::any("any", &NOT_ENABLED_OR_OPAQUE_CHILDREN);
static PURE_ENABLED: ConditionDescriptor = ConditionDescriptor::callback("pure", &PURE);

struct ComponentC;
struct ComponentD;
struct ComponentE;

fn catalog() -> ConditionCatalog {
    let registry = registry(vec![
        component::<ComponentA>("a", factory).with_condition(&OPAQUE),
        component::<ComponentB>("b", factory).with_condition(&FOLLOWS_A),
        component::<ComponentC>("c", factory).with_condition(&DISABLED_AND_OPAQUE),
        component::<ComponentD>("d", factory).with_condition(&NOT_ENABLED_OR_OPAQUE),
        component::<ComponentE>("e", factory).with_condition(&PURE_ENABLED),
    ]);

    ConditionCatalog::new(&registry, facts()).expect("catalog validates")
}

#[test]
fn tooling_runs_pure_callbacks_and_never_trusted_ones() {
    let catalog = catalog();
    let trusted_before = TRUSTED_CALLS.load(Ordering::SeqCst);
    let pure_before = PURE_CALLS.load(Ordering::SeqCst);

    let evaluation = catalog
        .evaluate_for_tooling(&snapshot(false, "safe"))
        .expect("tooling evaluates");

    assert_eq!(TRUSTED_CALLS.load(Ordering::SeqCst), trusted_before);
    assert!(PURE_CALLS.load(Ordering::SeqCst) > pure_before);
    assert_eq!(
        ["a", "b", "c", "d", "e"].map(|id| evaluation.component_outcome(id)),
        [
            Some(ConditionOutcome::NotEvaluated),
            Some(ConditionOutcome::NotEvaluated),
            Some(ConditionOutcome::Fails),
            Some(ConditionOutcome::Holds),
            Some(ConditionOutcome::Fails),
        ]
    );
    assert!(!evaluation.is_complete());
}

#[test]
fn only_components_whose_condition_holds_are_eligible() {
    let catalog = catalog();

    let evaluation = catalog
        .evaluate_for_tooling(&snapshot(true, "safe"))
        .expect("tooling evaluates");
    let eligible = evaluation
        .eligible_registry()
        .components
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();

    assert_eq!(eligible, ["e"]);
    assert_eq!(evaluation.component_eligible("a"), Some(false));
    assert_eq!(
        evaluation.component_outcome("c"),
        Some(ConditionOutcome::NotEvaluated)
    );
}

#[test]
fn decisions_record_undecided_nodes() {
    let catalog = catalog();

    let evaluation = catalog
        .evaluate_for_tooling(&snapshot(false, "safe"))
        .expect("tooling evaluates");
    let outcomes = evaluation
        .decisions()
        .iter()
        .filter(|decision| decision.component_id == "d")
        .map(|decision| (decision.condition_id, decision.outcome))
        .collect::<Vec<_>>();

    assert_eq!(
        outcomes,
        [
            ("any", ConditionOutcome::Holds),
            ("enabled", ConditionOutcome::Fails),
            ("not-enabled", ConditionOutcome::Holds),
            ("opaque", ConditionOutcome::NotEvaluated),
        ]
    );
}

#[test]
fn an_incomplete_evaluation_cannot_seed_a_reload() {
    let catalog = catalog();
    let evaluation = catalog
        .evaluate_for_tooling(&snapshot(false, "safe"))
        .expect("tooling evaluates");

    let error = catalog
        .evaluate_changed(&evaluation, &snapshot(true, "safe"))
        .expect_err("an undecided base is rejected");

    assert!(matches!(error, ConditionError::IncompleteEvaluation));
}

#[test]
fn runtime_evaluation_runs_trusted_callbacks() {
    let catalog = catalog();
    let trusted_before = TRUSTED_CALLS.load(Ordering::SeqCst);

    let evaluation = catalog
        .evaluate(&snapshot(false, "safe"))
        .expect("runtime evaluates");

    assert!(TRUSTED_CALLS.load(Ordering::SeqCst) > trusted_before);
    assert!(evaluation.is_complete());
    assert_eq!(evaluation.component_eligible("b"), Some(true));
}
