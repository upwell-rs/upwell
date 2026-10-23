//! Diagnostics and validation results.

use crate::*;

/// Diagnostic severity independent of process exit codes or transports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiagnosticSeverity {
    /// Informational context.
    Info,
    /// A recoverable concern.
    Warning,
    /// A validation or preparation failure.
    Error,
}

/// A stable typed diagnostic suitable for generic rendering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Stable namespaced diagnostic code.
    pub code: String,
    /// Severity category.
    pub severity: DiagnosticSeverity,
    /// Human-readable explanation.
    pub message: String,
    /// Related resource identities.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Related source positions.
    #[serde(default)]
    pub sources: Vec<SourceLocation>,
    /// Optional actionable correction.
    #[serde(default)]
    pub fix: Option<String>,
}

impl Default for Diagnostic {
    fn default() -> Self {
        Self {
            code: String::new(),
            severity: DiagnosticSeverity::Error,
            message: String::new(),
            resources: Vec::new(),
            sources: Vec::new(),
            fix: None,
        }
    }
}

/// A structural validation failure shared by document and probe-failure diagnostics.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticValidationError {
    /// A diagnostic code is not namespaced.
    #[error("diagnostic code '{code}' is not namespaced")]
    InvalidCode {
        /// Invalid diagnostic code.
        code: String,
    },
    /// A diagnostic has no user-facing explanation.
    #[error("diagnostic '{code}' has no message")]
    MissingMessage {
        /// Diagnostic code.
        code: String,
    },
    /// A diagnostic contains an empty resource identity.
    #[error("diagnostic '{code}' contains an empty resource identity")]
    EmptyResource {
        /// Diagnostic code.
        code: String,
    },
    /// A diagnostic contains an empty source file identity.
    #[error("diagnostic '{code}' contains an empty source file identity")]
    EmptySource {
        /// Diagnostic code.
        code: String,
    },
    /// A diagnostic source position is not one-based.
    #[error("diagnostic '{code}' contains invalid source coordinates")]
    InvalidSourcePosition {
        /// Diagnostic code.
        code: String,
    },
}

/// Transport-independent validation summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationResult {
    /// Whether validation completed without error diagnostics.
    pub valid: bool,
    /// Stable diagnostic codes in canonical order.
    pub diagnostic_codes: Vec<String>,
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self {
            valid: true,
            diagnostic_codes: Vec::new(),
        }
    }
}

pub(crate) fn validate_diagnostic(
    diagnostic: &Diagnostic,
) -> Result<(), DiagnosticValidationError> {
    if !is_namespaced(&diagnostic.code) {
        return Err(DiagnosticValidationError::InvalidCode {
            code: diagnostic.code.clone(),
        });
    }

    if is_blank(&diagnostic.message) {
        return Err(DiagnosticValidationError::MissingMessage {
            code: diagnostic.code.clone(),
        });
    }

    if diagnostic
        .resources
        .iter()
        .any(|resource| is_blank(resource))
    {
        return Err(DiagnosticValidationError::EmptyResource {
            code: diagnostic.code.clone(),
        });
    }

    for source in &diagnostic.sources {
        match validate_source_location(source) {
            Ok(()) => {}
            Err(IdentityValidationError::MissingSource) => {
                return Err(DiagnosticValidationError::EmptySource {
                    code: diagnostic.code.clone(),
                });
            }
            Err(_) => {
                return Err(DiagnosticValidationError::InvalidSourcePosition {
                    code: diagnostic.code.clone(),
                });
            }
        }
    }

    Ok(())
}

pub(crate) fn canonicalize_diagnostics(diagnostics: &mut [Diagnostic]) {
    for diagnostic in diagnostics.iter_mut() {
        sort_dedup(&mut diagnostic.resources);
        sort_dedup(&mut diagnostic.sources);
    }

    diagnostics.sort_by(|left, right| {
        (
            &left.code,
            &left.severity,
            &left.message,
            &left.resources,
            &left.sources,
            &left.fix,
        )
            .cmp(&(
                &right.code,
                &right.severity,
                &right.message,
                &right.resources,
                &right.sources,
                &right.fix,
            ))
    });
}

pub(crate) fn validation_result(diagnostics: &[Diagnostic]) -> ValidationResult {
    let mut diagnostic_codes: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.clone())
        .collect();
    let valid = diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error);

    diagnostic_codes.sort();
    diagnostic_codes.dedup();

    ValidationResult {
        valid,
        diagnostic_codes,
    }
}
