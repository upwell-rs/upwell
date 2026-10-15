//! Redaction, condition callbacks and descriptor source capture.

use super::*;

static MODE_EQUALS: ConditionDescriptor =
    ConditionDescriptor::config_equals("mode-equals", MODE, "private-canary");

#[test]
fn fact_values_and_literals_are_redacted() {
    let registry = ComponentRegistry {
        components: vec![component::<CustomAuthenticator>(
            "custom-authenticator",
            Some(&MODE_EQUALS),
        )],
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

fn config_callback(context: ConfigConditionContext<'_>) -> bool {
    context
        .get(ENABLED)
        .and_then(ConditionScalar::as_bool)
        .unwrap_or(false)
        && context.get(MODE).is_none()
}

static CONFIG_CALLBACK: ConfigConditionCallback = ConfigConditionCallback {
    kind: "test/config-enabled",
    inputs: &[ENABLED],
    evaluate: config_callback,
};

fn availability_callback(context: AvailabilityConditionContext<'_>) -> bool {
    context
        .eligible(AvailabilityTarget::Component("a"))
        .unwrap_or(false)
}

static AVAILABILITY_CALLBACK: AvailabilityConditionCallback = AvailabilityConditionCallback {
    kind: "test/component-a",
    inputs: &[AvailabilityTarget::Component("a")],
    evaluate: availability_callback,
};

static CONFIG_CALLBACK_CONDITION: ConditionDescriptor = ConditionDescriptor::new(
    "config-callback",
    ConditionPredicate::ConfigCallback(&CONFIG_CALLBACK),
);

static AVAILABILITY_CALLBACK_CONDITION: ConditionDescriptor = ConditionDescriptor::new(
    "availability-callback",
    ConditionPredicate::AvailabilityCallback(&AVAILABILITY_CALLBACK),
);

#[test]
fn callbacks_can_read_only_declared_metadata_inputs() {
    let registry = ComponentRegistry {
        components: vec![
            component::<ComponentA>("a", Some(&CONFIG_CALLBACK_CONDITION)),
            component::<ComponentB>("b", Some(&AVAILABILITY_CALLBACK_CONDITION)),
        ],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let evaluation = catalog
        .evaluate(&snapshot(true, "hidden-from-callback"))
        .expect("callbacks evaluate");

    assert_eq!(evaluation.component_eligible("a"), Some(true));
    assert_eq!(evaluation.component_eligible("b"), Some(true));
    assert_eq!(
        catalog.dependencies("a").expect("component is known"),
        [ConditionDependency::Config(ENABLED)]
    );
    assert_eq!(
        catalog.dependencies("b").expect("component is known"),
        [ConditionDependency::Component("a")]
    );
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
