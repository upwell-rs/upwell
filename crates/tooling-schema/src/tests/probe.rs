use super::*;

#[test]
fn probe_envelopes_round_trip_target_identity_for_success_and_failure() {
    let identity = DocumentIdentity {
        application: String::from("fixture"),
        package: Some(PackageIdentity {
            name: String::from("fixture-package"),
            version: Some(String::from("1.2.3")),
            manifest_path: Some(String::from("/workspace/fixture/Cargo.toml")),
        }),
        binary: Some(BinaryTargetIdentity {
            name: String::from("fixture-bin"),
        }),
        source: Some(SourceLocation {
            file: String::from("src/main.rs"),
            line: Some(17),
            column: Some(5),
            rust_path: None,
        }),
        workspace_root: None,
    };
    let success = ProbeEnvelope::success(fixture(), identity.clone());
    let failure = ProbeEnvelope::failure(
        identity.clone(),
        ProbeFailure {
            phase: Some(String::from("prepare")),
            diagnostics: vec![Diagnostic {
                code: String::from("upwell/tooling-prepare"),
                message: String::from("prepare failed"),
                severity: DiagnosticSeverity::Error,
                resources: vec![String::from("component:worker")],
                sources: Vec::new(),
                fix: Some(String::from("fix the fixture")),
            }],
            resource_kinds: BTreeMap::new(),
        },
    );

    for envelope in [success, failure] {
        let json = envelope.to_json().expect("probe envelope serializes");
        let decoded = ProbeEnvelope::from_json(&json).expect("probe envelope deserializes");
        let canonical_json = decoded.to_json().expect("decoded envelope serializes");

        assert_eq!(canonical_json, json);
        assert_eq!(decoded.identity, identity);
    }

    let success = ProbeEnvelope::success(fixture(), identity);
    let ProbeOutcome::Success { document } = success.outcome else {
        panic!("success envelope contains a failure");
    };

    assert_eq!(document.identity, success.identity);
}

#[test]
fn probe_boundary_rejects_schema_identity_and_success_document_mismatches() {
    let identity = probe_identity();
    let mut wrong_schema = ProbeEnvelope::failure(identity.clone(), failure_fixture());
    let mut missing_identity = ProbeEnvelope::failure(identity.clone(), failure_fixture());
    let mut mismatched = ProbeEnvelope::success(fixture(), identity.clone());

    wrong_schema.schema = semver::Version::new(1, 0, 0);
    missing_identity.identity.binary = None;

    let ProbeOutcome::Success { document } = &mut mismatched.outcome else {
        panic!("success fixture unexpectedly failed");
    };

    document.identity.application = String::from("other");

    assert!(matches!(
        wrong_schema.validate(),
        Err(ProbeValidationError::IncompatibleSchema { actual, .. })
            if actual == semver::Version::new(1, 0, 0)
    ));
    assert!(matches!(
        missing_identity.validate(),
        Err(ProbeValidationError::Identity(_))
    ));
    assert!(matches!(
        mismatched.validate(),
        Err(ProbeValidationError::IdentityMismatch)
    ));
}

#[test]
fn probe_target_identity_rejects_blank_invoker_values() {
    for (package, binary) in [(" ", "fixture-bin"), ("fixture-package", "\t")] {
        assert!(
            crate::ProbeTargetIdentity::new(
                PackageIdentity {
                    name: package.to_string(),
                    version: Some(String::from("1.2.3")),
                    manifest_path: Some(String::from("/workspace/fixture/Cargo.toml")),
                },
                BinaryTargetIdentity {
                    name: binary.to_string(),
                },
            )
            .is_err()
        );
    }
}

#[test]
fn probe_identity_rejects_invalid_optional_metadata_and_source_coordinates() {
    for package in [
        PackageIdentity {
            name: String::from("fixture-package"),
            version: Some(String::from(" ")),
            manifest_path: Some(String::from("/workspace/fixture/Cargo.toml")),
        },
        PackageIdentity {
            name: String::from("fixture-package"),
            version: Some(String::from("1.2.3")),
            manifest_path: Some(String::from("Cargo.toml")),
        },
    ] {
        assert!(
            crate::ProbeTargetIdentity::new(
                package,
                BinaryTargetIdentity {
                    name: String::from("fixture-bin"),
                },
            )
            .is_err()
        );
    }

    for (line, column) in [(Some(0), Some(1)), (Some(1), Some(0))] {
        let mut identity = probe_identity();

        identity.source = Some(SourceLocation {
            file: String::from("src/main.rs"),
            line,
            column,
            rust_path: None,
        });

        assert!(
            ProbeEnvelope::failure(identity, failure_fixture())
                .validate()
                .is_err()
        );
    }
}

#[test]
fn probe_identity_accepts_portable_absolute_manifest_paths() {
    for manifest_path in [
        "/workspace/fixture/Cargo.toml",
        r"C:\workspace\fixture\Cargo.toml",
        r"\\server\workspace\fixture\Cargo.toml",
    ] {
        crate::ProbeTargetIdentity::new(
            PackageIdentity {
                name: String::from("fixture-package"),
                version: Some(String::from("1.2.3")),
                manifest_path: Some(manifest_path.to_string()),
            },
            BinaryTargetIdentity {
                name: String::from("fixture-bin"),
            },
        )
        .expect("producer-native absolute manifest path is portable schema metadata");
    }
}

#[test]
fn hostile_probe_json_cannot_bypass_identity_validation() {
    let mut value =
        serde_json::to_value(ProbeEnvelope::failure(probe_identity(), failure_fixture()))
            .expect("probe envelope serializes");

    value["identity"]["package"]["manifest_path"] = json!("Cargo.toml");
    value["identity"]["source"]["line"] = json!(0);

    let encoded = serde_json::to_string(&value).expect("hostile envelope serializes");

    assert!(matches!(
        ProbeEnvelope::from_json(&encoded),
        Err(crate::ProbeDecodeError::Validate(
            ProbeValidationError::Identity(_)
        ))
    ));
}

#[test]
fn probe_emission_validates_and_canonicalizes_failure_diagnostics() {
    let identity = probe_identity();
    let mut failure = failure_fixture();

    failure.diagnostics[0].resources = vec![
        String::from("type:z"),
        String::from("type:a"),
        String::from("type:z"),
    ];

    let envelope = ProbeEnvelope::failure(identity, failure);
    let json = envelope.to_json().expect("valid failure emits");
    let decoded = ProbeEnvelope::from_json(&json).expect("emitted failure validates");
    let ProbeOutcome::Failure { failure } = decoded.outcome else {
        panic!("failure fixture unexpectedly succeeded");
    };

    assert_eq!(failure.diagnostics[0].resources, ["type:a", "type:z"]);
}

#[test]
fn failure_diagnostics_allow_resources_absent_without_a_success_document() {
    let mut failure = failure_fixture();

    failure.diagnostics[0].resources = vec![String::from("component:not-projected")];
    failure.diagnostics[0].sources = vec![SourceLocation {
        file: String::from("src/main.rs"),
        line: Some(10),
        column: None,
        rust_path: None,
    }];

    ProbeEnvelope::failure(probe_identity(), failure)
        .validate()
        .expect("failure diagnostics are structurally valid without a document graph");
}
