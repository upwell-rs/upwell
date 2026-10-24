use super::*;

#[test]
fn diagnostic_resource_classification_preserves_unknown_identities() {
    assert_eq!(
        diagnostic_resource_kind("provider:fixture::Service"),
        ResourceKind::Provider
    );
    assert_eq!(
        diagnostic_resource_kind("condition:custom:when"),
        ResourceKind::Condition
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
        RelationshipKind::Gates,
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

#[test]
fn conditions_gate_components_and_parent_conditions() {
    let mut document = fixture();

    document.resources.extend(
        ["condition:worker:when", "condition:worker:when/child"].map(|id| Resource {
            id: id.to_string(),
            kind: ResourceKind::Condition,
            name: String::from("when"),
            ..Resource::default()
        }),
    );
    document.relationships.extend([
        Relationship {
            kind: RelationshipKind::Gates,
            from: String::from("condition:worker:when"),
            to: String::from("component:worker"),
            labels: BTreeMap::new(),
        },
        Relationship {
            kind: RelationshipKind::Gates,
            from: String::from("condition:worker:when/child"),
            to: String::from("condition:worker:when"),
            labels: BTreeMap::new(),
        },
    ]);

    document
        .validate()
        .expect("conditions gate components and conditions");
}
