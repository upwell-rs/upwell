use std::path::PathBuf;

use upwell_tooling_schema::{
    BinaryTargetIdentity, DocumentIdentity, PackageIdentity, ProbeEnvelope, ProbeFailure,
    SourceLocation,
};

use super::matches_selected_target;
use crate::SelectedTarget;

#[test]
fn response_identity_must_match_every_cargo_owned_target_field() {
    let selected = selected_target();
    let envelope = envelope();

    assert!(matches_selected_target(&envelope, &selected));

    for mismatched in [
        SelectedTarget {
            package_name: String::from("other-package"),
            ..selected.clone()
        },
        SelectedTarget {
            package_version: String::from("2.0.0"),
            ..selected.clone()
        },
        SelectedTarget {
            manifest_path: PathBuf::from("/workspace/other/Cargo.toml"),
            ..selected.clone()
        },
        SelectedTarget {
            binary_name: String::from("other-binary"),
            ..selected.clone()
        },
    ] {
        assert!(!matches_selected_target(&envelope, &mismatched));
    }
}

fn selected_target() -> SelectedTarget {
    SelectedTarget {
        package_id: String::from("path+file:///workspace/app#1.0.0"),
        package_name: String::from("app"),
        package_version: String::from("1.0.0"),
        manifest_path: PathBuf::from("/workspace/app/Cargo.toml"),
        binary_name: String::from("app-server"),
        required_features: Vec::new(),
    }
}

fn envelope() -> ProbeEnvelope {
    ProbeEnvelope::failure(
        DocumentIdentity {
            application: String::from("app"),
            package: Some(PackageIdentity {
                name: String::from("app"),
                version: Some(String::from("1.0.0")),
                manifest_path: Some(String::from("/workspace/app/Cargo.toml")),
            }),
            binary: Some(BinaryTargetIdentity {
                name: String::from("app-server"),
            }),
            source: Some(SourceLocation {
                file: String::from("src/lib.rs"),
                line: Some(1),
                column: Some(1),
                rust_path: None,
            }),
            workspace_root: None,
        },
        ProbeFailure {
            phase: None,
            diagnostics: vec![upwell_tooling_schema::Diagnostic {
                code: String::from("test/failure"),
                severity: upwell_tooling_schema::DiagnosticSeverity::Error,
                message: String::from("fixture"),
                ..upwell_tooling_schema::Diagnostic::default()
            }],
            resource_kinds: Default::default(),
        },
    )
}
