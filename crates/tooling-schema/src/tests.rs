use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde_json::json;

use super::{
    BinaryTargetIdentity, CliArgument, CliCardinality, CliCommand, CliMetadata, CliOwner,
    CliProvider, CliProviderKind, Diagnostic, DiagnosticSeverity, DocumentIdentity, Facet,
    PackageIdentity, ProbeEnvelope, ProbeFailure, ProbeOutcome, ProbeValidationError, Provenance,
    Relationship, RelationshipKind, Resource, ResourceDisplay, ResourceKind, SourceLocation,
    TOOLING_SCHEMA_VERSION, ToolingDocument, ValidationError, ValidationResult, cli_provider_id,
    contribution_id, diagnostic_resource_kind, parse_cli_provider_id, parse_contribution_id,
};

#[test]
fn contribution_and_cli_provider_identities_round_trip_canonically() {
    let contributor = "plugin:test/worker";
    let contribution = "test/worker-cli";
    let resource = contribution_id(contributor, contribution);
    let provider = cli_provider_id(contributor, contribution);

    for (identity, parsed) in [
        (resource.as_str(), parse_contribution_id(&resource)),
        (provider.as_str(), parse_cli_provider_id(&provider)),
    ] {
        let parsed = parsed.expect("canonical identity parses");

        assert_eq!(parsed.contributor(), contributor, "{identity}");
        assert_eq!(parsed.contribution(), contribution, "{identity}");
    }

    assert!(parse_contribution_id(&provider).is_none());
    assert!(parse_cli_provider_id(&resource).is_none());
    assert!(parse_cli_provider_id("cli-provider:plugin:test/worker:").is_none());
}

#[test]
fn diagnostic_resource_classification_preserves_unknown_identities() {
    assert_eq!(
        diagnostic_resource_kind("provider:fixture::Service"),
        ResourceKind::Provider
    );
    assert_eq!(
        diagnostic_resource_kind("cli-command:serve"),
        ResourceKind::Unknown
    );
    assert_eq!(
        diagnostic_resource_kind("diagnostic:unattributed:0000"),
        ResourceKind::Unknown
    );
}

#[test]
fn declarative_resource_display_round_trips_and_validates_text() {
    let mut document = fixture();
    let resource = document
        .resources
        .first_mut()
        .expect("fixture has a resource");

    resource.display = Some(ResourceDisplay {
        label: Some(String::from("Human label")),
        group: Some(String::from("Protocol resources")),
        summary: Some(String::from("Human summary")),
        details: BTreeMap::from([(String::from("method"), String::from("GET"))]),
    });

    let json = document
        .to_canonical_json()
        .expect("display document emits");
    let decoded: ToolingDocument = serde_json::from_str(&json).expect("display document decodes");

    decoded
        .validate()
        .expect("decoded display document validates");

    document.canonicalize();

    assert_eq!(decoded, document);

    document.resources[0].display = Some(ResourceDisplay {
        label: Some(String::from("  ")),
        ..ResourceDisplay::default()
    });

    assert!(matches!(
        document.validate(),
        Err(ValidationError::BlankResourceDisplayText { .. })
    ));
}

fn fixture() -> ToolingDocument {
    let mut document = ToolingDocument::new(
        "0.20.0",
        DocumentIdentity {
            application: String::from("fixture"),
            ..DocumentIdentity::default()
        },
        "test/protocol",
    );

    document.resources = vec![
        Resource {
            id: String::from("type:test::Worker"),
            kind: ResourceKind::Type,
            name: String::from("Worker"),
            ..Resource::default()
        },
        Resource {
            id: String::from("component:worker"),
            kind: ResourceKind::Component,
            name: String::from("Worker"),
            ..Resource::default()
        },
    ];
    document.relationships.push(Relationship {
        kind: RelationshipKind::Provides,
        from: String::from("component:worker"),
        to: String::from("type:test::Worker"),
        labels: BTreeMap::new(),
    });
    document.facets.insert(
        String::from("third-party/routes"),
        Facet {
            schema_version: 2,
            value: json!({"routes": [{"method": "GET", "path": "/health"}]}),
        },
    );

    document
}

#[test]
fn canonical_json_round_trips_namespaced_facets() {
    let document = fixture();
    let json = document
        .to_canonical_json()
        .expect("fixture emits canonical JSON");
    let decoded: ToolingDocument = serde_json::from_str(&json).expect("fixture deserializes");

    assert_eq!(decoded, {
        let mut expected = document;

        expected.canonicalize();

        expected
    });
    assert_eq!(
        decoded.facets["third-party/routes"].value,
        json!({"routes": [{"method": "GET", "path": "/health"}]})
    );
    assert_eq!(decoded.schema, TOOLING_SCHEMA_VERSION);
    assert_eq!(decoded.schema.to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn tooling_schema_compatibility_uses_semantic_version_requirements() {
    let document = fixture();
    let envelope = ProbeEnvelope::success(document.clone(), probe_identity());
    let compatible = VersionReq::parse(&format!(
        "^{}.{}",
        TOOLING_SCHEMA_VERSION.major, TOOLING_SCHEMA_VERSION.minor
    ))
    .expect("compatible requirement parses");
    let incompatible = VersionReq::parse(&format!(">={}", TOOLING_SCHEMA_VERSION.major + 1))
        .expect("incompatible requirement parses");

    assert_eq!(document.schema, envelope.schema);
    assert!(document.schema_matches(&compatible));
    assert!(envelope.schema_matches(&compatible));
    assert!(!document.schema_matches(&incompatible));
    assert!(!envelope.schema_matches(&incompatible));
}

#[test]
fn schema_validation_accepts_current_and_later_minor_versions() {
    let compatible_versions = [
        TOOLING_SCHEMA_VERSION.clone(),
        Version::new(
            TOOLING_SCHEMA_VERSION.major,
            TOOLING_SCHEMA_VERSION.minor + 1,
            0,
        ),
    ];

    for version in compatible_versions {
        let mut document = fixture();
        let mut envelope = ProbeEnvelope::failure(probe_identity(), failure_fixture());

        document.schema = version.clone();
        envelope.schema = version.clone();

        document
            .validate()
            .unwrap_or_else(|error| panic!("schema {version} should be compatible: {error}"));
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("schema {version} should be compatible: {error}"));
    }
}

#[test]
fn schema_validation_rejects_next_major_and_older_minor_versions() {
    let incompatible_versions = [
        Version::new(TOOLING_SCHEMA_VERSION.major + 1, 0, 0),
        Version::new(
            TOOLING_SCHEMA_VERSION.major,
            TOOLING_SCHEMA_VERSION
                .minor
                .checked_sub(1)
                .expect("the current schema has an older minor release"),
            0,
        ),
    ];

    for version in incompatible_versions {
        let mut document = fixture();
        let mut envelope = ProbeEnvelope::failure(probe_identity(), failure_fixture());

        document.schema = version.clone();
        envelope.schema = version.clone();

        assert!(matches!(
            document.validate(),
            Err(ValidationError::IncompatibleSchema { actual, .. }) if actual == version
        ));
        assert!(matches!(
            envelope.validate(),
            Err(ProbeValidationError::IncompatibleSchema { actual, .. }) if actual == version
        ));
    }
}

#[test]
fn package_version_parser_rejects_metadata_instead_of_dropping_it() {
    assert_eq!(
        super::parse_package_version("12.34.56"),
        semver::Version::new(12, 34, 56)
    );
    assert!(std::panic::catch_unwind(|| super::parse_package_version("1.2.3-rc.1")).is_err());
    assert!(std::panic::catch_unwind(|| super::parse_package_version("1.2.3+build.1")).is_err());
}

#[test]
fn canonical_fixture_is_stable_across_process_boundaries() {
    const EXPECTED: &str = concat!(
        "{\"schema\":\"",
        env!("CARGO_PKG_VERSION"),
        "\",\"framework_version\":\"0.20.0\",",
        "\"identity\":{\"application\":\"fixture\",\"package\":null,\"binary\":null,\"source\":null},",
        "\"protocol\":\"test/protocol\",\"cli\":null,\"resources\":[",
        "{\"id\":\"component:worker\",\"kind\":\"component\",\"name\":\"Worker\",\"provenance\":null,\"labels\":{},\"facets\":{}},",
        "{\"id\":\"type:test::Worker\",\"kind\":\"type\",\"name\":\"Worker\",\"provenance\":null,\"labels\":{},\"facets\":{}}],",
        "\"relationships\":[{\"kind\":\"provides\",\"from\":\"component:worker\",\"to\":\"type:test::Worker\",\"labels\":{}}],",
        "\"diagnostics\":[],\"validation\":{\"valid\":true,\"diagnostic_codes\":[]},",
        "\"facets\":{\"third-party/routes\":{\"schema_version\":2,\"value\":{\"routes\":[{\"method\":\"GET\",\"path\":\"/health\"}]}}}}"
    );
    let emitted = fixture()
        .to_canonical_json()
        .expect("fixture emits canonical JSON");
    let decoded: ToolingDocument = serde_json::from_str(EXPECTED).expect("fixture boundary parses");

    assert_eq!(emitted, EXPECTED);
    assert_eq!(
        decoded
            .to_canonical_json()
            .expect("boundary fixture re-emits"),
        EXPECTED
    );
}

#[test]
fn equivalent_declaration_permutations_emit_identical_json() {
    let left = fixture();
    let mut right = fixture();

    right.resources.reverse();

    assert_eq!(
        left.to_canonical_json().expect("left fixture emits"),
        right.to_canonical_json().expect("right fixture emits")
    );
}

#[test]
fn additive_fields_are_compatible_in_both_directions() {
    let current = fixture()
        .to_canonical_json()
        .expect("current fixture emits");
    let mut newer: serde_json::Value = serde_json::from_str(&current).expect("JSON parses");

    newer["future_document_field"] = json!({"enabled": true});
    newer["resources"][0]["future_resource_field"] = json!("new");

    let decoded: ToolingDocument =
        serde_json::from_value(newer).expect("older client ignores additive fields");

    decoded
        .validate()
        .expect("decoded newer document validates");

    let old = json!({
        "schema": TOOLING_SCHEMA_VERSION.to_string(),
        "framework_version": "0.19.0",
        "identity": {"application": "old"},
        "protocol": "test/protocol"
    });
    let mut decoded_old: ToolingDocument =
        serde_json::from_value(old).expect("new client defaults additive fields");

    decoded_old.canonicalize();
    decoded_old
        .validate()
        .expect("defaulted old document validates");
    assert!(decoded_old.resources.is_empty());
    assert!(decoded_old.cli.is_none());
    assert!(decoded_old.validation.valid);
}

#[test]
fn validation_summary_is_deterministic() {
    let mut document = fixture();

    document.diagnostics = vec![
        Diagnostic {
            code: String::from("test/warning"),
            severity: DiagnosticSeverity::Warning,
            message: String::from("warning"),
            ..Diagnostic::default()
        },
        Diagnostic {
            code: String::from("test/error"),
            severity: DiagnosticSeverity::Error,
            message: String::from("error"),
            ..Diagnostic::default()
        },
    ];
    document.canonicalize();

    assert!(!document.validation.valid);
    assert_eq!(
        document.validation.diagnostic_codes,
        [String::from("test/error"), String::from("test/warning")]
    );
}

#[test]
fn required_semantic_fields_cannot_be_defaulted_by_hostile_json() {
    let resource = json!({"name": "missing identity"});
    let relationship = json!({"labels": {}});
    let diagnostic = json!({"resources": [], "sources": []});
    let source = json!({"line": 1});
    let provider = json!({"id": "cli-provider:test", "kind": "args"});
    let document = json!({
        "framework_version": "0.20.0",
        "protocol": "test/protocol"
    });

    assert!(serde_json::from_value::<Resource>(resource).is_err());
    assert!(serde_json::from_value::<Relationship>(relationship).is_err());
    assert!(serde_json::from_value::<Diagnostic>(diagnostic).is_err());
    assert!(serde_json::from_value::<SourceLocation>(source).is_err());
    assert!(serde_json::from_value::<CliProvider>(provider).is_err());
    assert!(serde_json::from_value::<ToolingDocument>(document).is_err());
}

#[test]
fn diagnostic_validation_requires_message_and_source_file() {
    let mut document = fixture();

    document.diagnostics.push(Diagnostic {
        code: String::from("test/empty-message"),
        severity: DiagnosticSeverity::Error,
        message: String::from(" "),
        ..Diagnostic::default()
    });
    document.canonicalize();

    assert!(matches!(
        document.validate(),
        Err(ValidationError::Diagnostic(
            super::DiagnosticValidationError::MissingMessage { .. }
        ))
    ));

    document.diagnostics[0].message = String::from("message");
    document.diagnostics[0].sources.push(SourceLocation {
        file: String::from(" "),
        line: None,
        column: None,
        rust_path: None,
    });

    assert!(matches!(
        document.validate(),
        Err(ValidationError::Diagnostic(
            super::DiagnosticValidationError::EmptySource { .. }
        ))
    ));
}

#[test]
fn diagnostic_and_provenance_sources_require_one_based_coordinates() {
    let mut diagnostic = fixture();

    diagnostic.diagnostics.push(Diagnostic {
        code: String::from("test/invalid-source"),
        severity: DiagnosticSeverity::Error,
        message: String::from("invalid source"),
        sources: vec![SourceLocation {
            file: String::from("src/main.rs"),
            line: Some(0),
            column: Some(1),
            rust_path: None,
        }],
        ..Diagnostic::default()
    });
    diagnostic.canonicalize();

    assert!(matches!(
        diagnostic.validate(),
        Err(ValidationError::Diagnostic(
            super::DiagnosticValidationError::InvalidSourcePosition { .. }
        ))
    ));

    let mut provenance = fixture();

    provenance.resources[0].provenance = Some(Provenance {
        source: Some(SourceLocation {
            file: String::from("src/main.rs"),
            line: Some(1),
            column: Some(0),
            rust_path: None,
        }),
        ..Provenance::default()
    });

    assert!(matches!(
        provenance.validate(),
        Err(ValidationError::InvalidProvenanceSource { .. })
    ));
}

#[test]
fn inconsistent_serialized_validation_summary_is_rejected() {
    let mut document = fixture();

    document.validation = ValidationResult {
        valid: false,
        diagnostic_codes: vec![String::from("hostile/invented")],
    };

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InconsistentValidationResult { .. })
    ));
}

#[test]
fn diagnostic_nested_permutations_and_all_fields_canonicalize_stably() {
    let first_source = SourceLocation {
        file: String::from("src/first.rs"),
        line: Some(1),
        column: Some(2),
        rust_path: None,
    };
    let second_source = SourceLocation {
        file: String::from("src/second.rs"),
        line: Some(3),
        column: Some(4),
        rust_path: None,
    };
    let mut left = fixture();
    let mut right = fixture();
    let diagnostic = Diagnostic {
        code: String::from("test/permuted"),
        severity: DiagnosticSeverity::Warning,
        message: String::from("same outer fields"),
        resources: vec![
            String::from("type:test::Worker"),
            String::from("component:worker"),
            String::from("type:test::Worker"),
        ],
        sources: vec![second_source.clone(), first_source.clone(), second_source],
        fix: Some(String::from("choose the first fix")),
    };
    let discriminator = Diagnostic {
        fix: Some(String::from("choose the second fix")),
        sources: vec![first_source],
        ..diagnostic.clone()
    };

    left.diagnostics = vec![diagnostic.clone(), discriminator.clone()];
    right.diagnostics = vec![discriminator, diagnostic];
    right.diagnostics[1].resources.reverse();
    right.diagnostics[1].sources.reverse();

    assert_eq!(
        left.to_canonical_json().expect("left emits"),
        right.to_canonical_json().expect("right emits")
    );
}

#[test]
fn resource_facets_require_owner_qualified_identities() {
    let mut document = fixture();

    document.resources[0].facets.insert(
        String::from("other-owner/routes"),
        Facet {
            schema_version: 1,
            value: json!({}),
        },
    );

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidFacetOwner { .. })
    ));
}

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
fn document_boundary_rejects_incompatible_package_schema_versions() {
    let mut document = fixture();

    document.schema = semver::Version::new(1, 0, 0);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::IncompatibleSchema { actual, .. })
            if actual == semver::Version::new(1, 0, 0)
    ));
}

#[test]
fn probe_target_identity_rejects_blank_invoker_values() {
    for (package, binary) in [(" ", "fixture-bin"), ("fixture-package", "\t")] {
        assert!(
            super::ProbeTargetIdentity::new(
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
            super::ProbeTargetIdentity::new(
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
        super::ProbeTargetIdentity::new(
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
        Err(super::ProbeDecodeError::Validate(
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

fn probe_identity() -> DocumentIdentity {
    DocumentIdentity {
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
    }
}

fn failure_fixture() -> ProbeFailure {
    ProbeFailure {
        phase: Some(String::from("prepare")),
        diagnostics: vec![Diagnostic {
            code: String::from("upwell/tooling-prepare"),
            message: String::from("Application planning failed."),
            severity: DiagnosticSeverity::Error,
            resources: Vec::new(),
            sources: Vec::new(),
            fix: None,
        }],
        resource_kinds: BTreeMap::new(),
    }
}

fn cli_fixture() -> CliMetadata {
    CliMetadata {
        root: CliCommand {
            name: String::from("fixture"),
            arguments: vec![
                CliArgument {
                    id: String::from("input"),
                    index: Some(1),
                    aliases: vec![String::from("z"), String::from("a")],
                    visible_aliases: vec![String::from("visible")],
                    value_names: vec![String::from("FIRST"), String::from("REST")],
                    cardinality: CliCardinality {
                        min_values: 1,
                        max_values: None,
                        repeatable: true,
                    },
                    owner: CliOwner::Plugin {
                        provider: String::from("cli-provider:plugin:test/plugin:test/args"),
                    },
                    ..CliArgument::default()
                },
                CliArgument {
                    id: String::from("profile"),
                    long: Some(String::from("profile")),
                    owner: CliOwner::Framework,
                    ..CliArgument::default()
                },
            ],
            commands: vec![CliCommand {
                name: String::from("inspect"),
                owner: CliOwner::Application,
                ..CliCommand::default()
            }],
            owner: CliOwner::Application,
            ..CliCommand::default()
        },
        default_command: None,
        providers: vec![CliProvider {
            id: String::from("cli-provider:plugin:test/plugin:test/args"),
            contributor: String::from("plugin:test/plugin"),
            contribution: String::from("test/args"),
            kind: CliProviderKind::Args,
        }],
    }
}

#[test]
fn typed_cli_round_trips_aliases_cardinality_nesting_and_provenance() {
    let mut document = fixture();

    add_cli_resources(&mut document);
    document.cli = Some(cli_fixture());

    let json = document
        .to_canonical_json()
        .expect("typed CLI document emits");
    let decoded: ToolingDocument = serde_json::from_str(&json).expect("typed CLI document decodes");
    let cli = decoded.cli.expect("CLI section exists");
    let input = &cli.root.arguments[0];

    assert_eq!(input.aliases, ["a", "z"]);
    assert_eq!(input.visible_aliases, ["visible"]);
    assert_eq!(input.value_names, ["FIRST", "REST"]);
    assert!(input.default_values.is_empty());
    assert_eq!(input.cardinality.max_values, None);
    assert!(input.cardinality.repeatable);
    assert!(matches!(cli.root.arguments[1].owner, CliOwner::Framework));
    assert!(json.contains("\"owner\":{\"kind\":\"framework\"}"));
    assert!(!json.contains("\"slot\""));
    assert_eq!(cli.root.commands[0].name, "inspect");
    assert!(matches!(input.owner, CliOwner::Plugin { .. }));
}

#[test]
fn typed_cli_provider_and_declaration_permutations_canonicalize_identically() {
    let mut left = fixture();
    let mut right = fixture();
    let mut cli = cli_fixture();
    let second = CliProvider {
        id: String::from("cli-provider:plugin:test/plugin:test/commands"),
        contributor: String::from("plugin:test/plugin"),
        contribution: String::from("test/commands"),
        kind: CliProviderKind::CommandSet,
    };

    cli.providers.push(second);
    cli.root.commands.push(CliCommand {
        name: String::from("admin"),
        owner: CliOwner::Application,
        ..CliCommand::default()
    });
    add_cli_resources(&mut left);
    add_cli_resources(&mut right);
    add_second_cli_provider_resource(&mut left);
    add_second_cli_provider_resource(&mut right);
    left.cli = Some(cli.clone());
    cli.providers.reverse();
    cli.root.commands.reverse();
    cli.root.arguments[0].aliases.reverse();
    right.cli = Some(cli);

    assert_eq!(
        left.to_canonical_json().expect("left emits"),
        right.to_canonical_json().expect("right emits")
    );
}

#[test]
fn typed_cli_validation_rejects_unknown_provider_and_invalid_cardinality() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.arguments[0].cardinality = CliCardinality {
        min_values: 2,
        max_values: Some(1),
        repeatable: false,
    };
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliCardinality { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers.clear();
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::UnknownCliProvider { .. })
    ));
}

#[test]
fn typed_cli_validation_rejects_duplicate_provider_identities() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.providers.push(cli.providers[0].clone());
    document.cli = Some(cli);

    assert!(matches!(
        document.to_canonical_json(),
        Err(super::EmitError::Validation(
            ValidationError::DuplicateCliProvider { .. }
        ))
    ));
}

#[test]
fn cli_provider_identity_contributor_and_resource_must_correspond() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.providers[0].contribution.clear();
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliProviderContribution { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers[0].contributor = String::from("plugin:test/missing");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliProviderIdentity { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers[0].id = String::from("cli-provider:plugin:test/missing:test/args");
    cli.providers[0].contributor = String::from("plugin:test/missing");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliProviderContributor { .. })
    ));
}

#[test]
fn typed_cli_validation_rejects_blank_identities_and_parser_namespace_collisions() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.name = String::from(" \t");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliCommandName { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[0].id = String::from(" ");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliArgumentId { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[1].short = Some(' ');
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliParserName { .. })
    ));

    for (argument, expected) in [
        (
            CliArgument {
                id: String::from("long-collision"),
                long: Some(String::from("profile")),
                owner: CliOwner::Application,
                ..CliArgument::default()
            },
            "long",
        ),
        (
            CliArgument {
                id: String::from("short-collision"),
                short: Some('p'),
                owner: CliOwner::Application,
                ..CliArgument::default()
            },
            "short",
        ),
    ] {
        let mut cli = cli_fixture();

        cli.root.arguments[1].short = Some('p');
        cli.root.arguments.push(argument);
        document.cli = Some(cli);

        match (expected, document.validate()) {
            ("long", Err(ValidationError::DuplicateCliLongOption { .. }))
            | ("short", Err(ValidationError::DuplicateCliShortOption { .. })) => {}
            (_, result) => panic!("unexpected namespace validation result: {result:?}"),
        }
    }
}

#[test]
fn typed_cli_validation_checks_aliases_inherited_globals_and_default_command_ids() {
    let mut document = fixture();

    add_cli_resources(&mut document);

    let mut cli = cli_fixture();

    cli.root.commands[0].aliases = vec![String::from("duplicate")];
    cli.root.commands.push(CliCommand {
        name: String::from("other"),
        visible_aliases: vec![String::from("duplicate")],
        owner: CliOwner::Application,
        ..CliCommand::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::DuplicateCliCommand { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[1].global = true;
    cli.root.commands[0].arguments.push(CliArgument {
        id: String::from("nested-profile"),
        visible_aliases: vec![String::from("profile")],
        owner: CliOwner::Application,
        ..CliArgument::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::DuplicateCliLongOption { .. })
    ));

    let mut cli = cli_fixture();

    cli.default_command = Some(String::from("serve"));
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::UnknownDefaultCliCommand { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.commands[0].id = Some(String::from("serve"));
    cli.default_command = Some(String::from("serve"));
    document.cli = Some(cli);

    document
        .validate()
        .expect("canonical default command resolves independently of its parser name");
}

#[test]
fn typed_cli_validation_rejects_zero_positional_index() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.arguments.push(CliArgument {
        id: String::from("invalid-position"),
        index: Some(0),
        owner: CliOwner::Application,
        ..CliArgument::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliPosition { .. })
    ));
}

#[test]
fn provenance_owner_must_exist_have_an_owner_kind_and_be_consistent() {
    let mut document = fixture();

    document.resources[0].provenance = Some(Provenance {
        owner: Some(String::from("plugin:missing")),
        ..Provenance::default()
    });

    assert!(matches!(
        document.validate(),
        Err(ValidationError::UnknownProvenanceOwner { .. })
    ));

    document.resources[0].provenance = Some(Provenance {
        owner: Some(String::from("component:worker")),
        ..Provenance::default()
    });

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidProvenanceOwnerKind { .. })
    ));

    document.resources[0].provenance = Some(Provenance {
        owner: Some(String::from("type:test::Worker")),
        ..Provenance::default()
    });

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidProvenanceOwnerKind { .. })
    ));
}

#[test]
fn duplicate_relationships_and_core_kind_mismatches_are_rejected() {
    let mut duplicate = fixture();

    duplicate.relationships.push(Relationship {
        labels: BTreeMap::from([(String::from("different"), String::from("label"))]),
        ..duplicate.relationships[0].clone()
    });

    assert!(matches!(
        duplicate.validate(),
        Err(ValidationError::DuplicateRelationship { .. })
    ));

    for kind in [
        RelationshipKind::OpensScope,
        RelationshipKind::Provides,
        RelationshipKind::Binds,
        RelationshipKind::Replaces,
        RelationshipKind::Suppresses,
        RelationshipKind::Hooks,
        RelationshipKind::Contains,
        RelationshipKind::Contributes,
    ] {
        let mut invalid = fixture();

        invalid.relationships[0].kind = kind;
        invalid.relationships[0].from = String::from("type:test::Worker");
        invalid.relationships[0].to = String::from("component:worker");

        assert!(matches!(
            invalid.validate(),
            Err(ValidationError::InvalidRelationshipKinds { .. })
        ));
    }
}

#[test]
fn contains_and_contributes_have_distinct_endpoint_contracts() {
    let mut document = fixture();

    document.resources.extend([
        Resource {
            id: String::from("protocol:test/protocol"),
            kind: ResourceKind::Protocol,
            name: String::from("test/protocol"),
            ..Resource::default()
        },
        Resource {
            id: String::from("protocol:test/protocol/tooling/route"),
            kind: ResourceKind::Contribution,
            name: String::from("route"),
            provenance: Some(Provenance {
                owner: Some(String::from("protocol:test/protocol")),
                ..Provenance::default()
            }),
            ..Resource::default()
        },
    ]);
    document.relationships.push(Relationship {
        kind: RelationshipKind::Contains,
        from: String::from("protocol:test/protocol"),
        to: String::from("protocol:test/protocol/tooling/route"),
        labels: BTreeMap::new(),
    });

    document.validate().expect("protocol containment is valid");

    let relationship = document
        .relationships
        .last_mut()
        .expect("relationship exists");

    relationship.kind = RelationshipKind::Contributes;

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidRelationshipKinds { .. })
    ));
}

fn add_cli_resources(document: &mut ToolingDocument) {
    document.resources.push(Resource {
        id: String::from("plugin:test/plugin"),
        kind: ResourceKind::Plugin,
        name: String::from("test/plugin"),
        ..Resource::default()
    });
}

fn add_second_cli_provider_resource(_document: &mut ToolingDocument) {}
