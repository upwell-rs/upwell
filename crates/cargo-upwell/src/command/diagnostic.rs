use cargo_metadata::diagnostic::DiagnosticLevel;
use upwell_tooling_schema::{Diagnostic, DiagnosticSeverity, SourceLocation};

use crate::{BuildError, BuildEvidence, DiscoveryError, ProbeError, ProbeEvidence, SelectionError};

pub(super) fn discovery_diagnostics(error: &DiscoveryError) -> Vec<Diagnostic> {
    match error {
        DiscoveryError::Selection(error) => vec![selection_diagnostic(error)],
        DiscoveryError::Cancelled => vec![tool_diagnostic(
            "cargo-upwell/metadata-cancelled",
            "Cargo workspace discovery was cancelled.",
            None,
        )],
        DiscoveryError::Failed { stderr, .. } => vec![tool_diagnostic(
            "cargo-upwell/metadata-failed",
            "Cargo could not load workspace metadata.",
            if stderr.is_empty() {
                None
            } else {
                Some(String::from(
                    "Cargo emitted additional stderr; correct the manifest error and retry.",
                ))
            },
        )],
        DiscoveryError::Launch(_) => vec![tool_diagnostic(
            "cargo-upwell/cargo-launch",
            "Cargo could not be launched for workspace discovery.",
            Some(String::from(
                "Verify Cargo is installed and the CARGO environment variable is correct.",
            )),
        )],
        DiscoveryError::OutputTooLarge => vec![tool_diagnostic(
            "cargo-upwell/metadata-output-too-large",
            "Cargo workspace metadata exceeded the supported size.",
            None,
        )],
        DiscoveryError::Encoding(_) | DiscoveryError::Metadata(_) => vec![tool_diagnostic(
            "cargo-upwell/metadata-invalid",
            "Cargo returned invalid workspace metadata.",
            Some(String::from("Update Cargo and retry the command.")),
        )],
        DiscoveryError::Process(_) | DiscoveryError::Capture => vec![tool_diagnostic(
            "cargo-upwell/metadata-process",
            "Cargo workspace discovery could not be monitored safely.",
            None,
        )],
    }
}

fn selection_diagnostic(error: &SelectionError) -> Diagnostic {
    let (code, message, resources, fix) = match error {
        SelectionError::NoPackage => (
            "cargo-upwell/no-package",
            "The Cargo workspace contains no binary application target.".to_string(),
            Vec::new(),
            Some(String::from(
                "Add a named Upwell application binary with the tooling feature enabled.",
            )),
        ),
        SelectionError::PackageNotFound {
            requested,
            candidates,
        } => (
            "cargo-upwell/package-not-found",
            format!("Cargo package '{requested}' was not found in this workspace."),
            candidates
                .iter()
                .map(|candidate| format!("cargo-package:{}", candidate.name))
                .collect(),
            Some(package_selection_hint(
                candidates.iter().map(|candidate| candidate.name.as_str()),
            )),
        ),
        SelectionError::AmbiguousPackage { candidates } => (
            "cargo-upwell/package-ambiguous",
            "More than one workspace package contains an eligible binary target.".to_string(),
            candidates
                .iter()
                .map(|candidate| format!("cargo-package:{}", candidate.name))
                .collect(),
            Some(package_selection_hint(
                candidates.iter().map(|candidate| candidate.name.as_str()),
            )),
        ),
        SelectionError::NoBinary { package } => (
            "cargo-upwell/no-binary",
            format!("Cargo package '{package}' contains no binary application target."),
            vec![format!("cargo-package:{package}")],
            Some(String::from(
                "Add a binary target containing a named Upwell application.",
            )),
        ),
        SelectionError::BinaryNotFound {
            package,
            requested,
            candidates,
        } => (
            "cargo-upwell/binary-not-found",
            format!("Binary target '{requested}' was not found in package '{package}'."),
            candidates
                .iter()
                .map(|candidate| format!("cargo-binary:{}:{package}", candidate.name))
                .collect(),
            Some(binary_selection_hint(
                package,
                candidates.iter().map(|candidate| candidate.name.as_str()),
            )),
        ),
        SelectionError::RequiredFeatures {
            package,
            binary,
            missing_features,
        } => (
            "cargo-upwell/required-features",
            format!("Binary target '{binary}' requires disabled package features."),
            vec![format!("cargo-binary:{binary}:{package}")],
            Some(format!(
                "Retry with --features {}.",
                missing_features.join(",")
            )),
        ),
        SelectionError::AmbiguousBinary {
            package,
            candidates,
        } => (
            "cargo-upwell/binary-ambiguous",
            format!("Package '{package}' contains multiple eligible binary targets."),
            candidates
                .iter()
                .map(|candidate| format!("cargo-binary:{}:{package}", candidate.name))
                .collect(),
            Some(binary_selection_hint(
                package,
                candidates.iter().map(|candidate| candidate.name.as_str()),
            )),
        ),
    };

    Diagnostic {
        code: code.to_string(),
        severity: DiagnosticSeverity::Error,
        message,
        resources,
        sources: Vec::new(),
        fix,
    }
}

pub(super) fn build_diagnostics(error: &BuildError) -> Vec<Diagnostic> {
    let evidence = build_error_evidence(error);
    let mut diagnostics = evidence
        .map(|evidence| {
            evidence
                .diagnostics
                .iter()
                .map(|diagnostic| Diagnostic {
                    code: diagnostic.diagnostic.code.as_ref().map_or_else(
                        || String::from("cargo-upwell/rustc"),
                        |code| format!("rustc/{}", code.code),
                    ),
                    severity: rustc_severity(diagnostic.diagnostic.level),
                    message: diagnostic.diagnostic.message.clone(),
                    resources: vec![format!("cargo-target:{}", diagnostic.target_name)],
                    sources: primary_sources(&diagnostic.diagnostic),
                    fix: None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    diagnostics.push(tool_diagnostic(
        "cargo-upwell/build-failed",
        error.to_string(),
        build_error_hint(error, evidence),
    ));

    diagnostics
}

fn rustc_severity(level: DiagnosticLevel) -> DiagnosticSeverity {
    match level {
        DiagnosticLevel::Ice | DiagnosticLevel::Error | DiagnosticLevel::FailureNote => {
            DiagnosticSeverity::Error
        }
        DiagnosticLevel::Warning => DiagnosticSeverity::Warning,
        DiagnosticLevel::Note | DiagnosticLevel::Help => DiagnosticSeverity::Info,
        _ => DiagnosticSeverity::Info,
    }
}

fn build_error_evidence(error: &BuildError) -> Option<&BuildEvidence> {
    match error {
        BuildError::Cancelled { evidence }
        | BuildError::Message { evidence, .. }
        | BuildError::OutputTooLarge { evidence }
        | BuildError::Failed { evidence }
        | BuildError::MissingBuildFinished { evidence }
        | BuildError::MissingExecutable { evidence }
        | BuildError::AmbiguousExecutable { evidence, .. }
        | BuildError::InvalidExecutable { evidence, .. } => Some(evidence),
        BuildError::Launch(_) | BuildError::Process(_) | BuildError::Capture => None,
    }
}

fn build_error_hint(error: &BuildError, evidence: Option<&BuildEvidence>) -> Option<String> {
    match error {
        BuildError::Launch(_) => Some(String::from(
            "Verify Cargo is installed and the CARGO environment variable is correct.",
        )),
        BuildError::Failed { .. } => evidence.and_then(|evidence| {
            (!evidence.stderr.is_empty()).then(|| {
                String::from(
                    "Cargo emitted additional stderr; correct the build failure and retry.",
                )
            })
        }),
        BuildError::MissingExecutable { .. } => Some(String::from(
            "Verify the selected target is an executable binary for the host platform.",
        )),
        _ => None,
    }
}

pub(super) fn probe_diagnostics(error: &ProbeError) -> Vec<Diagnostic> {
    let (code, message, fix) = match error {
        ProbeError::MissingResponse { evidence } => (
            "cargo-upwell/probe-missing",
            "The selected binary did not publish a generated tooling response.",
            Some(if evidence.stderr.is_empty() {
                String::from(
                    "Use a named app! application, enable its tooling feature, and select the binary containing its generated process entry.",
                )
            } else {
                String::from(
                    "The application emitted additional stderr. Use a named app! application, enable its tooling feature, and select the binary containing its generated process entry.",
                )
            }),
        ),
        ProbeError::Decode { .. } => (
            "cargo-upwell/probe-incompatible",
            "The selected binary returned an incompatible tooling response.",
            Some(String::from(
                "Install the cargo-upwell version matching the application's Upwell dependencies.",
            )),
        ),
        ProbeError::TargetIdentityMismatch { .. } => (
            "cargo-upwell/probe-target-mismatch",
            "The tooling response belongs to a different Cargo target.",
            Some(String::from(
                "Clean private tooling artifacts and retry the command.",
            )),
        ),
        ProbeError::StatusMismatch { .. } => (
            "cargo-upwell/probe-status-mismatch",
            "The application process status disagrees with its tooling response.",
            Some(String::from(
                "Update the application and cargo-upwell to matching versions.",
            )),
        ),
        ProbeError::Cancelled { .. } => (
            "cargo-upwell/probe-cancelled",
            "The application tooling probe was cancelled.",
            None,
        ),
        ProbeError::Launch(_) => (
            "cargo-upwell/probe-launch",
            "The selected application executable could not be launched.",
            Some(String::from(
                "Use a host-compatible build target; cross-target runners are not supported.",
            )),
        ),
        ProbeError::ResponseTooLarge { .. } => (
            "cargo-upwell/probe-response-too-large",
            "The application tooling response exceeded the supported size.",
            None,
        ),
        ProbeError::ResponseEncoding { .. } | ProbeError::ReadResponse { .. } => (
            "cargo-upwell/probe-response-invalid",
            "The application tooling response could not be read as JSON.",
            None,
        ),
        ProbeError::CreateDirectory(_)
        | ProbeError::DirectoryExhausted
        | ProbeError::ResponseMetadata { .. } => (
            "cargo-upwell/probe-storage",
            "Cargo Upwell could not use its private probe directory.",
            Some(String::from(
                "Verify the Cargo target directory is writable.",
            )),
        ),
        ProbeError::Process(_) | ProbeError::Capture => (
            "cargo-upwell/probe-process",
            "The application tooling probe could not be monitored safely.",
            None,
        ),
    };

    vec![tool_diagnostic(code, message, fix)]
}

pub(super) fn append_evidence_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    build: &BuildEvidence,
    probe: &ProbeEvidence,
    cleanup_error: Option<&std::io::Error>,
) {
    if build.stdout_truncated || build.stderr_truncated {
        diagnostics.push(warning_diagnostic(
            "cargo-upwell/build-output-truncated",
            "Cargo build evidence exceeded the retained output limit.",
        ));
    }

    if probe.stdout_truncated || probe.stderr_truncated {
        diagnostics.push(warning_diagnostic(
            "cargo-upwell/probe-output-truncated",
            "Application probe evidence exceeded the retained output limit.",
        ));
    }

    if cleanup_error.is_some() {
        diagnostics.push(warning_diagnostic(
            "cargo-upwell/probe-cleanup",
            "A private probe directory could not be removed after validation.",
        ));
    }
}

fn primary_sources(diagnostic: &cargo_metadata::diagnostic::Diagnostic) -> Vec<SourceLocation> {
    diagnostic
        .spans
        .iter()
        .filter(|span| span.is_primary)
        .filter_map(|span| {
            let line = u32::try_from(span.line_start).ok()?;
            let column = u32::try_from(span.column_start).ok()?;

            Some(SourceLocation {
                file: span.file_name.clone(),
                line: Some(line),
                column: Some(column),
                rust_path: None,
            })
        })
        .collect()
}

fn package_selection_hint<'a>(candidates: impl Iterator<Item = &'a str>) -> String {
    format!(
        "Select one package with --package <name>. Candidates: {}.",
        candidates.collect::<Vec<_>>().join(", ")
    )
}

fn binary_selection_hint<'a>(package: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    format!(
        "Select one binary with --package {package} --bin <name>. Candidates: {}.",
        candidates.collect::<Vec<_>>().join(", ")
    )
}

pub(super) fn tool_diagnostic(
    code: impl Into<String>,
    message: impl Into<String>,
    fix: Option<String>,
) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: DiagnosticSeverity::Error,
        message: message.into(),
        resources: Vec::new(),
        sources: Vec::new(),
        fix,
    }
}

fn warning_diagnostic(code: &str, message: &str) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        severity: DiagnosticSeverity::Warning,
        message: message.to_string(),
        resources: Vec::new(),
        sources: Vec::new(),
        fix: None,
    }
}

#[cfg(test)]
mod tests;
