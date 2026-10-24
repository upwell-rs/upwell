use super::*;

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
            crate::DiagnosticValidationError::MissingMessage { .. }
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
            crate::DiagnosticValidationError::EmptySource { .. }
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
            crate::DiagnosticValidationError::InvalidSourcePosition { .. }
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
