//! Redaction, condition callbacks and descriptor source capture.

use super::*;

static MODE_EQUALS: ConditionDescriptor =
    ConditionDescriptor::config_equals("mode-equals", MODE, "private-canary");

#[test]
fn fact_values_and_literals_are_redacted() {
    let registry = ComponentRegistry {
        components: vec![
            component::<CustomAuthenticator>("custom-authenticator", factory)
                .with_condition(&MODE_EQUALS),
        ],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");
    let fact_snapshot = snapshot(false, "runtime-canary");

    let events = crate::test_support::capture_events(|| {
        catalog.evaluate(&fact_snapshot).expect("facts evaluate");
    });
    let evaluation = catalog.evaluate(&fact_snapshot).expect("facts evaluate");
    let rendered = format!("{fact_snapshot:?} {evaluation:?} {MODE_EQUALS:?}");
    let rendered_events = format!("{events:?}");

    assert!(!rendered.contains("runtime-canary"));
    assert!(!rendered.contains("private-canary"));
    assert!(!rendered_events.contains("runtime-canary"));
    assert!(!rendered_events.contains("private-canary"));
    assert!(events.iter().any(|event| {
        event.target == crate::observability::CONDITION_TARGET
            && event
                .fields
                .get("event_name")
                .is_some_and(|value| value == "condition-node")
            && event
                .fields
                .get("condition_id")
                .is_some_and(|value| value == "mode-equals")
    }));
}

fn fact_and_eligibility(context: ConditionContext<'_>) -> bool {
    context
        .fact(ENABLED)
        .and_then(ConditionScalar::as_bool)
        .unwrap_or(false)
        && context.eligible("a").unwrap_or(false)
        && context.fact(MODE).is_none()
}

static FACT_AND_ELIGIBILITY: ConditionCallback = ConditionCallback {
    kind: "test/enabled-and-a",
    inputs: &[
        ConditionInputId::Fact(ENABLED),
        ConditionInputId::Component("a"),
    ],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: fact_and_eligibility,
};

static FACT_AND_ELIGIBILITY_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("enabled-and-a", &FACT_AND_ELIGIBILITY);

#[test]
fn callbacks_read_declared_facts_and_eligibility_in_one_expression() {
    let registry = registry(vec![
        component::<ComponentA>("a", factory),
        component::<ComponentB>("b", factory).with_condition(&FACT_AND_ELIGIBILITY_CONDITION),
    ]);
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let enabled = catalog
        .evaluate(&snapshot(true, "hidden-from-callback"))
        .expect("callbacks evaluate");
    let disabled = catalog
        .evaluate(&snapshot(false, "hidden-from-callback"))
        .expect("callbacks evaluate");

    assert_eq!(enabled.component_eligible("b"), Some(true));
    assert_eq!(disabled.component_eligible("b"), Some(false));
    assert_eq!(
        catalog.dependencies("b").expect("component is known"),
        [
            ConditionDependency::Config(ENABLED),
            ConditionDependency::Component("a")
        ]
    );
}

fn reads_undeclared_path(_: ConditionContext<'_>) -> bool {
    scope::raw_input("features.hidden").is_none()
}

static UNDECLARED_READ: ConditionCallback = ConditionCallback {
    kind: "test/undeclared-read",
    inputs: &[],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: reads_undeclared_path,
};

static UNDECLARED_READ_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("undeclared-read", &UNDECLARED_READ);

#[test]
fn reading_an_undeclared_raw_path_fails_the_evaluation() {
    let registry = registry(vec![
        component::<ComponentA>("a", factory).with_condition(&UNDECLARED_READ_CONDITION),
    ]);
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let error = catalog
        .evaluate(&snapshot(true, "safe"))
        .expect_err("an undeclared read rejects the candidate");

    assert!(matches!(
        error,
        ConditionError::CallbackInputRejected {
            component_id: "a",
            condition_id: "undeclared-read",
            rejection: ConditionInputRejection::Undeclared("features.hidden"),
        }
    ));
}

static DESCRIBED_EXPRESSION: ConditionExpression = ConditionExpression {
    text: "AuthConfig.mode == \"custom\"",
    reads: &[ConditionInputRead {
        input: ConditionInputId::Fact(MODE),
        field: "",
        source: SOURCE,
    }],
};

static MISDESCRIBED: ConditionCallback = ConditionCallback {
    kind: "test/misdescribed",
    inputs: &[ConditionInputId::Fact(ENABLED)],
    expression: Some(&DESCRIBED_EXPRESSION),
    purity: ConditionPurity::Trusted,
    evaluate: fact_and_eligibility,
};

static MISDESCRIBED_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("misdescribed", &MISDESCRIBED);

#[test]
fn expression_reads_must_be_declared_inputs() {
    let registry = registry(vec![
        component::<ComponentA>("a", factory).with_condition(&MISDESCRIBED_CONDITION),
    ]);

    let error = match ConditionCatalog::new(&registry, facts()) {
        Ok(_) => panic!("an undeclared expression read validates"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        ConditionError::UndeclaredExpressionRead {
            component_id: "a",
            condition_id: "misdescribed",
            input: ConditionInputId::Fact(MODE),
        }
    ));
}

fn eligible_b(context: ConditionContext<'_>) -> bool {
    context.eligible("b").unwrap_or(false)
}

static READS_B: ConditionCallback = ConditionCallback {
    kind: "test/reads-b",
    inputs: &[ConditionInputId::Component("b")],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: eligible_b,
};

static READS_B_CONDITION: ConditionDescriptor = ConditionDescriptor::callback("reads-b", &READS_B);
static B_READS_A: ConditionDescriptor = ConditionDescriptor::component_eligible("b-reads-a", "a");

#[test]
fn eligibility_inputs_of_callbacks_join_the_availability_graph() {
    let registry = registry(vec![
        component::<ComponentA>("a", factory).with_condition(&READS_B_CONDITION),
        component::<ComponentB>("b", factory).with_condition(&B_READS_A),
    ]);

    let error = match ConditionCatalog::new(&registry, facts()) {
        Ok(_) => panic!("a cycle through a callback validates"),
        Err(error) => error,
    };

    assert!(matches!(error, ConditionError::AvailabilityCycle(_)));
    assert!(error.to_string().contains("reads-b"));
    assert!(error.to_string().contains("b-reads-a"));
}

#[test]
fn source_macro_captures_the_call_site() {
    const {
        assert!(SOURCE.line > 0);
        assert!(SOURCE.column > 0);
    }

    assert_eq!(
        std::path::Path::new(SOURCE.file).file_name(),
        Some(std::ffi::OsStr::new("mod.rs"))
    );
}
