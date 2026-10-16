//! Typed binding and raw-path callback inputs, and their change detection.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

struct AuthSettings {
    custom: bool,
}

const AUTH: ConfigBindingId = ConfigBindingId::of::<AuthSettings>("auth");
const BETA: &str = "features.beta";

/// Declares a callback over the `auth` binding and the `features.beta` raw path that counts
/// its evaluations, so each test observes its own re-evaluations.
macro_rules! counted_config_callback {
    ($condition:ident, $count:ident) => {
        static $count: AtomicUsize = AtomicUsize::new(0);

        static $condition: ConditionDescriptor = {
            fn evaluate(context: ConditionContext<'_>) -> bool {
                $count.fetch_add(1, Ordering::SeqCst);

                custom_and_beta(context)
            }

            static CALLBACK: ConditionCallback = ConditionCallback {
                kind: "test/custom-and-beta",
                inputs: &[ConditionInputId::Binding(AUTH), ConditionInputId::Raw(BETA)],
                expression: None,
                evaluate,
            };

            ConditionDescriptor::callback("custom-and-beta", &CALLBACK)
        };
    };
}

fn custom_and_beta(context: ConditionContext<'_>) -> bool {
    let custom = context
        .binding::<AuthSettings>("auth")
        .is_some_and(|auth| auth.custom);
    let beta = scope::raw_input(BETA).is_some_and(|value| value.downcast_ref() == Some(&true));

    custom && beta
}

fn auth(custom: bool) -> Arc<dyn std::any::Any + Send + Sync> {
    Arc::new(AuthSettings { custom })
}

fn config_snapshot(
    auth: &Arc<dyn std::any::Any + Send + Sync>,
    beta: Option<bool>,
) -> ConditionFactSnapshot {
    let mut snapshot = snapshot(false, "safe");

    snapshot
        .insert_binding(AUTH, Arc::clone(auth))
        .expect("binding is unique");
    snapshot.insert_raw(BETA, beta).expect("raw path is unique");

    snapshot
}

fn catalog_for(condition: &'static ConditionDescriptor) -> ConditionCatalog {
    let registry = registry(vec![component::<ComponentA>("a", Some(condition))]);

    ConditionCatalog::new(&registry, facts()).expect("catalog validates")
}

#[test]
fn callbacks_read_typed_bindings_and_raw_paths() {
    counted_config_callback!(CONDITION, EVALUATIONS);
    let catalog = catalog_for(&CONDITION);
    let on = auth(true);
    let off = auth(false);

    let both = catalog
        .evaluate(&config_snapshot(&on, Some(true)))
        .expect("inputs evaluate");
    let binding_off = catalog
        .evaluate(&config_snapshot(&off, Some(true)))
        .expect("inputs evaluate");
    let raw_absent = catalog
        .evaluate(&config_snapshot(&on, None))
        .expect("an absent raw path evaluates");

    assert_eq!(both.component_eligible("a"), Some(true));
    assert_eq!(binding_off.component_eligible("a"), Some(false));
    assert_eq!(raw_absent.component_eligible("a"), Some(false));
    assert_eq!(
        catalog.dependencies("a").expect("component is known"),
        [
            ConditionDependency::Binding(AUTH),
            ConditionDependency::Raw(BETA)
        ]
    );
    assert_eq!(
        registry(vec![component::<ComponentA>("a", Some(&CONDITION))]).condition_config_inputs(),
        [ConditionInputId::Binding(AUTH), ConditionInputId::Raw(BETA)].into()
    );
}

#[test]
fn a_declared_but_unsupplied_config_input_is_reported() {
    counted_config_callback!(CONDITION, EVALUATIONS);
    let catalog = catalog_for(&CONDITION);
    let mut missing_raw = snapshot(false, "safe");

    missing_raw
        .insert_binding(AUTH, auth(true))
        .expect("binding is unique");

    let unbound = catalog.evaluate(&snapshot(false, "safe"));
    let unresolved = catalog.evaluate(&missing_raw);

    assert!(matches!(
        unbound,
        Err(ConditionError::MissingConfigInput(input)) if input == ConditionInputId::Binding(AUTH)
    ));
    assert!(matches!(
        unresolved,
        Err(ConditionError::MissingConfigInput(input)) if input == ConditionInputId::Raw(BETA)
    ));
    assert_eq!(EVALUATIONS.load(Ordering::SeqCst), 0);
}

#[test]
fn unchanged_config_inputs_reuse_the_previous_decision() {
    counted_config_callback!(CONDITION, EVALUATIONS);
    let catalog = catalog_for(&CONDITION);
    let committed = auth(true);
    let previous = catalog
        .evaluate(&config_snapshot(&committed, Some(true)))
        .expect("inputs evaluate");

    let unchanged = catalog
        .evaluate_changed(&previous, &config_snapshot(&committed, Some(true)))
        .expect("unchanged inputs evaluate");

    assert_eq!(EVALUATIONS.load(Ordering::SeqCst), 1);
    assert_eq!(unchanged.component_eligible("a"), Some(true));
}

#[test]
fn a_changed_binding_or_raw_value_re_evaluates_the_condition() {
    counted_config_callback!(CONDITION, EVALUATIONS);
    let catalog = catalog_for(&CONDITION);
    let committed = auth(true);
    let previous = catalog
        .evaluate(&config_snapshot(&committed, Some(true)))
        .expect("inputs evaluate");

    let rebound = catalog
        .evaluate_changed(&previous, &config_snapshot(&auth(false), Some(true)))
        .expect("a changed binding evaluates");
    let raw_changed = catalog
        .evaluate_changed(&previous, &config_snapshot(&committed, Some(false)))
        .expect("a changed raw value evaluates");

    assert_eq!(EVALUATIONS.load(Ordering::SeqCst), 3);
    assert_eq!(rebound.component_eligible("a"), Some(false));
    assert_eq!(raw_changed.component_eligible("a"), Some(false));
}

#[test]
fn snapshot_debug_output_names_inputs_without_values() {
    let rendered = format!("{:?}", config_snapshot(&auth(true), Some(true)));

    assert!(rendered.contains("features.beta"));
    assert!(rendered.contains("auth"));
    assert!(!rendered.contains("true"));
}
