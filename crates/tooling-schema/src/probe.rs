//! The target-local probe envelope and its outcome.

use crate::*;

/// Probe-specific context and diagnostics returned by a target-local tooling probe.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeFailure {
    /// Lifecycle phase that failed, when the probe reached application lifecycle work.
    #[serde(default)]
    pub phase: Option<String>,
    /// One or more authoritative schema diagnostics describing the failure.
    pub diagnostics: Vec<Diagnostic>,
    /// Authoritative generic kinds for diagnostic resource identities when known.
    #[serde(default)]
    pub resource_kinds: BTreeMap<String, ResourceKind>,
}

/// Success or failure payload emitted by a target-local tooling probe.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum ProbeOutcome {
    /// Preparation and projection completed successfully.
    Success {
        /// Projected immutable application plan.
        document: Box<ToolingDocument>,
    },
    /// Catalog resolution, lifecycle preparation, or projection failed.
    Failure {
        /// Stable typed failure payload.
        failure: ProbeFailure,
    },
}

/// Versioned process envelope exchanged with a selected generated application target.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProbeEnvelope {
    /// Envelope compatibility version.
    pub schema: Version,
    /// Target identity retained even when preparation fails.
    pub identity: DocumentIdentity,
    /// Probe result.
    #[serde(flatten)]
    pub outcome: ProbeOutcome,
}

impl ProbeEnvelope {
    /// Creates a successful envelope and synchronizes its target identity with the document.
    pub fn success(mut document: ToolingDocument, identity: DocumentIdentity) -> Self {
        document.identity = identity.clone();

        Self {
            schema: TOOLING_SCHEMA_VERSION,
            identity,
            outcome: ProbeOutcome::Success {
                document: Box::new(document),
            },
        }
    }

    /// Creates a failed envelope with target identity and a stable typed failure.
    pub fn failure(identity: DocumentIdentity, failure: ProbeFailure) -> Self {
        Self {
            schema: TOOLING_SCHEMA_VERSION,
            identity,
            outcome: ProbeOutcome::Failure { failure },
        }
    }

    /// Records the Cargo workspace root on the envelope and its document identity.
    ///
    /// Set by the Cargo-side invoker, which alone knows the workspace the target was built in.
    pub fn set_workspace_root(&mut self, workspace_root: impl Into<String>) {
        let workspace_root = workspace_root.into();

        if let ProbeOutcome::Success { document } = &mut self.outcome {
            document.identity.workspace_root = Some(workspace_root.clone());
        }

        self.identity.workspace_root = Some(workspace_root);
    }

    /// Whether the target prepared and projected successfully.
    pub const fn is_success(&self) -> bool {
        matches!(self.outcome, ProbeOutcome::Success { .. })
    }

    /// Whether this envelope's schema satisfies a consumer's semantic-version requirement.
    pub fn schema_matches(&self, requirement: &VersionReq) -> bool {
        requirement.matches(&self.schema)
    }

    /// Validates the envelope, its payload, and cross-boundary identity invariants.
    pub fn validate(&self) -> Result<(), ProbeValidationError> {
        if !schema_requirement().matches(&self.schema) {
            return Err(ProbeValidationError::IncompatibleSchema {
                actual: self.schema.clone(),
                required: schema_requirement(),
            });
        }

        validate_document_identity(&self.identity)?;

        match &self.outcome {
            ProbeOutcome::Success { document } => {
                document.validate()?;

                if document.identity != self.identity {
                    return Err(ProbeValidationError::IdentityMismatch);
                }
            }
            ProbeOutcome::Failure { failure } => validate_probe_failure(failure)?,
        }

        Ok(())
    }

    /// Parses and validates one process probe envelope.
    pub fn from_json(json: &str) -> Result<Self, ProbeDecodeError> {
        let envelope: Self = serde_json::from_str(json)?;

        envelope.validate()?;

        Ok(envelope)
    }

    /// Emits compact canonical JSON for the process probe contract after validation.
    pub fn to_json(&self) -> Result<String, ProbeEmitError> {
        let mut envelope = self.clone();

        if let ProbeOutcome::Success { document } = &mut envelope.outcome {
            document.canonicalize();
        }

        if let ProbeOutcome::Failure { failure } = &mut envelope.outcome {
            canonicalize_diagnostics(&mut failure.diagnostics);
        }

        envelope.validate()?;

        Ok(serde_json::to_string(&envelope)?)
    }
}

/// A structural validation failure at the probe envelope boundary.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ProbeValidationError {
    /// The envelope belongs to an incompatible tooling-schema release.
    #[error("incompatible probe envelope schema {actual}; expected {required}")]
    IncompatibleSchema {
        /// Encountered schema version.
        actual: Version,
        /// Compatible schema releases.
        required: VersionReq,
    },
    /// Target or declaration identity is incomplete.
    #[error(transparent)]
    Identity(#[from] IdentityValidationError),
    /// A successful document violates the public tooling schema.
    #[error(transparent)]
    Document(#[from] ValidationError),
    /// A successful envelope and document disagree about the selected target.
    #[error("probe envelope identity does not match its successful document")]
    IdentityMismatch,
    /// A failed envelope contains an empty phase.
    #[error("probe failure contains an empty lifecycle phase")]
    EmptyFailurePhase,
    /// A failed envelope contains no diagnostic.
    #[error("probe failure contains no diagnostics")]
    MissingFailureDiagnostic,
    /// A failed envelope contains a structurally invalid diagnostic.
    #[error(transparent)]
    Diagnostic(#[from] DiagnosticValidationError),
}

/// A typed failure while decoding and validating a probe envelope.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeDecodeError {
    /// JSON did not decode as a probe envelope.
    #[error("failed to decode the tooling probe envelope: {0}")]
    Deserialize(#[from] serde_json::Error),
    /// Decoded data violated the probe contract.
    #[error(transparent)]
    Validate(#[from] ProbeValidationError),
}

/// A typed failure while validating or serializing a probe envelope.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeEmitError {
    /// The envelope violates the process contract.
    #[error(transparent)]
    Validate(#[from] ProbeValidationError),
    /// A validated envelope could not be represented as JSON.
    #[error("failed to serialize the tooling probe envelope: {0}")]
    Serialize(#[from] serde_json::Error),
}

fn validate_probe_failure(failure: &ProbeFailure) -> Result<(), ProbeValidationError> {
    if failure.phase.as_ref().is_some_and(|phase| is_blank(phase)) {
        return Err(ProbeValidationError::EmptyFailurePhase);
    }

    if failure.diagnostics.is_empty() {
        return Err(ProbeValidationError::MissingFailureDiagnostic);
    }

    for diagnostic in &failure.diagnostics {
        validate_diagnostic(diagnostic)?;
    }

    Ok(())
}
