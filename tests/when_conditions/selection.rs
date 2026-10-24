//! Provider selection and eligibility driven by `#[when]` conditions.

use upwell::condition::scope::ConditionInputRejection;
use upwell::{AppError, ConditionError};

use crate::app::{
    AuditExporter, AuditTransport, AuthGate, CustomAuthenticator, NamedCondition,
    ReorderedAuditExporter, Settings, build, write_settings,
};

const DEFAULT: Settings = Settings {
    mode: "default",
    audit: false,
    export: None,
};

const CUSTOM_AUDITED: Settings = Settings {
    mode: "custom",
    audit: true,
    export: Some("true"),
};

#[tokio::test]
async fn the_unconditional_default_serves_while_no_condition_holds() {
    let (_dir, _config_dir, app) = build(&DEFAULT).await;
    let app = app.expect("the app builds");
    let root = app.runtime().root();

    assert_eq!(
        root.get::<AuthGate>().expect("the gate resolves").scheme(),
        "default"
    );
    assert!(root.get::<CustomAuthenticator>().is_none());
    assert!(root.get::<AuditTransport>().is_none());
    assert!(root.get::<AuditExporter>().is_none());
    assert!(root.get::<NamedCondition>().is_none());
}

#[tokio::test]
async fn typed_bindings_eligibility_and_raw_paths_enable_their_components() {
    let (_dir, _config_dir, app) = build(&CUSTOM_AUDITED).await;
    let app = app.expect("the app builds");
    let root = app.runtime().root();

    assert_eq!(
        root.get::<AuthGate>().expect("the gate resolves").scheme(),
        "custom",
        "the eligible primary provider wins over the default"
    );
    assert!(root.get::<AuditTransport>().is_some());
    assert!(root.get::<AuditExporter>().is_some());
    assert!(
        root.get::<ReorderedAuditExporter>().is_some(),
        "stacking order does not change the outcome"
    );
    assert!(root.get::<NamedCondition>().is_some());
}

#[tokio::test]
async fn every_stacked_condition_must_hold() {
    let missing_export = Settings {
        export: None,
        ..CUSTOM_AUDITED
    };
    let disabled_export = Settings {
        export: Some("false"),
        ..CUSTOM_AUDITED
    };

    for settings in [missing_export, disabled_export] {
        let (_dir, _config_dir, app) = build(&settings).await;
        let app = app.expect("the app builds");
        let root = app.runtime().root();

        assert!(root.get::<AuditTransport>().is_some());
        assert!(root.get::<AuditExporter>().is_none());
        assert!(root.get::<ReorderedAuditExporter>().is_none());
    }
}

#[tokio::test]
async fn reloads_follow_the_conditions_in_both_directions() {
    let (_dir, config_dir, app) = build(&DEFAULT).await;
    let app = app.expect("the app builds");
    let runtime = app.runtime();

    write_settings(&config_dir, &CUSTOM_AUDITED);
    runtime
        .reload_config()
        .await
        .expect("enabling reload commits");

    let root = runtime.root();

    assert_eq!(root.get::<AuthGate>().expect("gate").scheme(), "custom");
    assert!(root.get::<AuditExporter>().is_some());

    write_settings(&config_dir, &DEFAULT);
    runtime
        .reload_config()
        .await
        .expect("disabling reload commits");

    let root = runtime.root();

    assert_eq!(root.get::<AuthGate>().expect("gate").scheme(), "default");
    assert!(root.get::<AuditExporter>().is_none());
}

#[tokio::test]
async fn a_raw_value_of_the_wrong_type_rejects_the_candidate() {
    let invalid = Settings {
        export: Some("\"sometimes\""),
        ..CUSTOM_AUDITED
    };

    let (_dir, _config_dir, app) = build(&invalid).await;
    let error = app.expect_err("an undeserializable raw input fails the build");

    let AppError::Condition(ConditionError::CallbackInputRejected {
        component_id,
        rejection,
        ..
    }) = error
    else {
        panic!("expected a rejected callback input, got {error:?}");
    };

    assert!(
        ["auditexporter", "reorderedauditexporter"].contains(&component_id),
        "the rejection names a raw-path reader, got {component_id}"
    );
    assert_eq!(
        rejection,
        ConditionInputRejection::Invalid {
            path: "features.audit_export",
            target: "bool",
        }
    );
}
