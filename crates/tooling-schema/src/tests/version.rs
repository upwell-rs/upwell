use super::*;

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
        crate::parse_package_version("12.34.56"),
        semver::Version::new(12, 34, 56)
    );
    assert!(std::panic::catch_unwind(|| crate::parse_package_version("1.2.3-rc.1")).is_err());
    assert!(std::panic::catch_unwind(|| crate::parse_package_version("1.2.3+build.1")).is_err());
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
