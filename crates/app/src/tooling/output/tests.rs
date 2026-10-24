use upwell_tooling_schema::{
    BinaryTargetIdentity, Diagnostic, DiagnosticSeverity, DocumentIdentity, PackageIdentity,
    ProbeEnvelope, ProbeFailure, SourceLocation,
};

use crate::test_support::TempFixture;

use super::{
    ToolingProbeOutputError, ToolingProbeOutputTargetError, emit_probe_envelope,
    emit_probe_envelope_with_hook,
};

#[test]
fn response_is_published_as_valid_canonical_json() {
    let fixture = TempFixture::new("upwell-tooling-output-publish");
    let path = fixture.child("response.json");

    emit_probe_envelope(&path, &envelope()).expect("response publishes");

    let json = std::fs::read_to_string(&path).expect("published response is readable");
    let decoded = ProbeEnvelope::from_json(json.trim_end()).expect("published response validates");

    assert!(!decoded.is_success());
    assert_eq!(
        directory_entries(fixture.path()),
        [String::from("response.json")]
    );
}

#[test]
fn invalid_envelope_is_rejected_before_target_inspection() {
    let fixture = TempFixture::new("upwell-tooling-output-serialize-first");
    let path = fixture.child("response.json");
    let mut envelope = envelope();

    std::fs::write(&path, "preserve-me").expect("existing fixture is written");
    envelope.identity.application.clear();

    assert!(matches!(
        emit_probe_envelope(&path, &envelope),
        Err(ToolingProbeOutputError::Serialize(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).expect("existing fixture remains readable"),
        "preserve-me"
    );
}

#[test]
fn existing_regular_target_is_never_intentionally_replaced() {
    let fixture = TempFixture::new("upwell-tooling-output-existing-file");
    let path = fixture.child("response.json");

    std::fs::write(&path, "preserve-me").expect("existing fixture is written");

    assert!(matches!(
        emit_probe_envelope(&path, &envelope()),
        Err(ToolingProbeOutputError::InvalidTarget {
            reason: ToolingProbeOutputTargetError::ExistingFile,
            ..
        })
    ));
    assert_eq!(
        std::fs::read_to_string(&path).expect("existing fixture remains readable"),
        "preserve-me"
    );
}

#[test]
fn raced_target_atomically_rejects_publication_and_cleans_temporary_file() {
    let fixture = TempFixture::new("upwell-tooling-output-publish-race-secret");
    let path = fixture.child("response.json");
    let attacker_content = "attacker-content";

    let error = emit_probe_envelope_with_hook(&path, &envelope(), |final_path| {
        std::fs::write(final_path, attacker_content).expect("raced target is created");
    })
    .expect_err("raced publication fails");

    assert!(matches!(
        &error,
        ToolingProbeOutputError::Publish { source, .. }
            if source.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(
        std::fs::read_to_string(&path).expect("raced target remains readable"),
        attacker_content
    );
    assert_eq!(
        directory_entries(fixture.path()),
        [String::from("response.json")]
    );
    assert!(!error.to_string().contains("publish-race-secret"));
    assert!(!format!("{error:?}").contains("publish-race-secret"));
}

#[test]
fn missing_parent_is_rejected_without_creating_directories() {
    let fixture = TempFixture::new("upwell-tooling-output-missing-parent");
    let directory = fixture.child("missing-parent");
    let path = directory.join("response.json");

    assert!(matches!(
        emit_probe_envelope(&path, &envelope()),
        Err(ToolingProbeOutputError::InvalidTarget {
            reason: ToolingProbeOutputTargetError::MissingParent,
            ..
        })
    ));
    assert!(!directory.exists());
}

#[cfg(unix)]
#[test]
fn published_response_is_private_and_symlink_targets_are_rejected() {
    use std::os::unix::fs::{MetadataExt as _, symlink};

    let fixture = TempFixture::new("upwell-tooling-output-private");
    let path = fixture.child("response.json");

    emit_probe_envelope(&path, &envelope()).expect("response publishes");

    assert_eq!(
        std::fs::metadata(&path)
            .expect("response metadata exists")
            .mode()
            & 0o777,
        0o600
    );

    let symlink_path = fixture.child("symlink.json");

    symlink(&path, &symlink_path).expect("response symlink is created");
    assert!(matches!(
        emit_probe_envelope(&symlink_path, &envelope()),
        Err(ToolingProbeOutputError::InvalidTarget {
            reason: ToolingProbeOutputTargetError::ExistingSymlink,
            ..
        })
    ));
}

fn envelope() -> ProbeEnvelope {
    ProbeEnvelope::failure(
        DocumentIdentity {
            application: String::from("output-test"),
            package: Some(PackageIdentity {
                name: String::from("upwell-app"),
                version: Some(String::from(env!("CARGO_PKG_VERSION"))),
                manifest_path: Some(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"))),
            }),
            binary: Some(BinaryTargetIdentity {
                name: String::from("output-test"),
            }),
            source: Some(SourceLocation {
                file: String::from(file!()),
                line: None,
                column: None,
                rust_path: None,
            }),
            workspace_root: None,
        },
        ProbeFailure {
            phase: Some(String::from("setup")),
            diagnostics: vec![Diagnostic {
                code: String::from("upwell/tooling-setup"),
                severity: DiagnosticSeverity::Error,
                message: String::from("Application setup failed during the tooling probe."),
                ..Diagnostic::default()
            }],
            resource_kinds: Default::default(),
        },
    )
}

fn directory_entries(path: &std::path::Path) -> Vec<String> {
    let mut entries = std::fs::read_dir(path)
        .expect("fixture directory is readable")
        .map(|entry| {
            entry
                .expect("fixture entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();

    entries.sort();

    entries
}
