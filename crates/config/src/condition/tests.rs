use std::sync::Arc;

use upwell_core::condition::scope::{self, ConditionInputRejection, RawConfigValue};

use super::*;
use crate::{ConfigManager, MapResolver};

const CONFIG: &str = r#"
[features]
beta = "${BETA}"
label = "beta-${BETA}"
limit = 3
"#;

fn manager(beta: &str) -> ConfigManager {
    let values = [("BETA".to_string(), beta.to_string())]
        .into_iter()
        .collect();

    ConfigManager::from_str(CONFIG)
        .expect("inline config parses")
        .with_resolvers(ResolverChain(vec![Box::new(MapResolver(values))]))
}

fn raw(manager: &ConfigManager, path: &str) -> Option<RawConfigValue> {
    manager
        .resolve_raw(path)
        .expect("raw path renders")
        .map(|resolved| Arc::new(resolved) as RawConfigValue)
}

/// Runs `read` in a condition scope answering `features` and `features.beta` from `manager`.
fn in_scope<R>(
    manager: &ConfigManager,
    read: impl FnOnce() -> R,
) -> (R, Option<ConditionInputRejection>) {
    scope::with_raw_inputs(
        vec![
            ("features", raw(manager, "features")),
            ("features.beta", raw(manager, "features.beta")),
            ("features.missing", raw(manager, "features.missing")),
        ],
        read,
    )
}

#[test]
fn resolve_coerces_full_placeholders_like_config_fields() {
    let manager = manager("true");

    let (beta, rejection) = in_scope(&manager, || resolve::<bool>("features.beta"));

    assert_eq!(beta, Some(true));
    assert_eq!(rejection, None);
}

#[test]
fn resolve_deserializes_whole_tables_and_answers_none_for_absent_paths() {
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Features {
        beta: bool,
        label: String,
        limit: u8,
    }

    let manager = manager("false");

    let ((features, missing), rejection) = in_scope(&manager, || {
        (
            resolve::<Features>("features"),
            resolve::<bool>("features.missing"),
        )
    });

    assert_eq!(
        features,
        Some(Features {
            beta: false,
            label: "beta-false".to_string(),
            limit: 3,
        })
    );
    assert_eq!(missing, None);
    assert_eq!(rejection, None);
}

#[test]
fn a_value_of_the_wrong_type_rejects_the_evaluation() {
    let manager = manager("not-a-bool");

    let (beta, rejection) = in_scope(&manager, || resolve::<bool>("features.beta"));

    assert_eq!(beta, None);
    assert_eq!(
        rejection,
        Some(ConditionInputRejection::Invalid {
            path: "features.beta",
            target: "bool",
        })
    );
}

#[test]
fn templated_strings_still_only_satisfy_string_targets() {
    let manager = manager("true");

    let (label, rejection) = in_scope(&manager, || {
        scope::with_raw_inputs(
            vec![("features.label", raw(&manager, "features.label"))],
            || resolve::<bool>("features.label"),
        )
    });

    assert_eq!(label.0, None);
    assert!(matches!(
        label.1,
        Some(ConditionInputRejection::Invalid { .. })
    ));
    assert_eq!(rejection, None);
}

#[test]
fn resolved_config_compares_rendered_values_and_redacts_debug_output() {
    let first = manager("true").resolve_raw("features").expect("renders");
    let same = manager("true").resolve_raw("features").expect("renders");
    let changed = manager("false").resolve_raw("features").expect("renders");

    assert_eq!(first, same);
    assert_ne!(first, changed);
    assert_eq!(format!("{first:?}"), "Some(ResolvedConfig(..))");
}
