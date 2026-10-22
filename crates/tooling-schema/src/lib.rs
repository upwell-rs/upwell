//! Versioned protocol-neutral documents exchanged with Upwell developer tooling.
//!
//! The schema contains only stable textual identities and JSON data. It deliberately has no
//! dependency on application runtime state, Clap, Cargo metadata, or a concrete protocol.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod preview;

pub use preview::*;

/// JSON value used by opaque protocol and plugin facets.
pub use serde_json::Value as JsonValue;

/// Tooling schema version published by this package.
pub const TOOLING_SCHEMA_VERSION: Version = parse_package_version(env!("CARGO_PKG_VERSION"));

/// Exact hidden process argument used to request one target-local tooling probe.
pub const TOOLING_PROBE_ARGUMENT: &str = "--__upwell-tooling-probe-v1";

/// Environment variable containing the response file path for a target-local tooling probe.
pub const TOOLING_PROBE_OUTPUT_ENV: &str = "UPWELL_TOOLING_PROBE_OUTPUT";

/// Environment variable containing the Cargo package name selected by the tooling invoker.
pub const TOOLING_PROBE_PACKAGE_NAME_ENV: &str = "UPWELL_TOOLING_PROBE_PACKAGE_NAME";

/// Environment variable containing the selected Cargo package version.
pub const TOOLING_PROBE_PACKAGE_VERSION_ENV: &str = "UPWELL_TOOLING_PROBE_PACKAGE_VERSION";

/// Environment variable containing the selected Cargo package manifest path.
pub const TOOLING_PROBE_MANIFEST_PATH_ENV: &str = "UPWELL_TOOLING_PROBE_MANIFEST_PATH";

/// Environment variable containing the Cargo binary target selected by the tooling invoker.
pub const TOOLING_PROBE_BINARY_NAME_ENV: &str = "UPWELL_TOOLING_PROBE_BINARY_NAME";

/// Document facet holding the JSON Schema (2020-12) of the whole configuration tree, with every
/// binding nested at its path and the template grammar under `x-upwell-templating`.
pub const CONFIG_SCHEMA_FACET: &str = "framework/config-schema";

/// Resource-local suffix of the facet holding one config binding's JSON Schema; the full facet
/// id is `<binding resource id>/config-schema`.
pub const CONFIG_BINDING_SCHEMA_FACET: &str = "config-schema";

/// Schema version of the [`CONFIG_SCHEMA_FACET`] and [`CONFIG_BINDING_SCHEMA_FACET`] payloads.
pub const CONFIG_SCHEMA_FACET_VERSION: u16 = 1;

const fn parse_package_version(version: &str) -> Version {
    let bytes = version.as_bytes();
    let mut components = [0_u64; 3];
    let mut component = 0_usize;
    let mut has_digit = false;
    let mut index = 0_usize;

    while index < bytes.len() {
        let byte = bytes[index];

        if byte >= b'0' && byte <= b'9' {
            components[component] = match components[component].checked_mul(10) {
                Some(value) => value,
                None => panic!("tooling schema package version component overflows u64"),
            };
            components[component] = match components[component].checked_add((byte - b'0') as u64) {
                Some(value) => value,
                None => panic!("tooling schema package version component overflows u64"),
            };
            has_digit = true;
        } else if byte == b'.' {
            assert!(
                has_digit && component < 2,
                "tooling schema package version must contain major, minor, and patch components"
            );

            component += 1;
            has_digit = false;
        } else {
            panic!("tooling schema package version must not contain prerelease or build metadata");
        }

        index += 1;
    }

    assert!(
        component == 2 && has_digit,
        "tooling schema package version must contain major, minor, and patch components"
    );

    Version::new(components[0], components[1], components[2])
}

fn schema_requirement() -> VersionReq {
    VersionReq::parse(&format!(
        ">={TOOLING_SCHEMA_VERSION},<{}.0.0",
        TOOLING_SCHEMA_VERSION.major + 1
    ))
    .expect("the package-derived tooling schema requirement is valid")
}

/// Cargo package identity, when a target-local entry supplies it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackageIdentity {
    /// Cargo package name.
    pub name: String,
    /// Cargo package version.
    pub version: Option<String>,
    /// Stable manifest path when available to the selected target.
    pub manifest_path: Option<String>,
}

/// Binary target identity, when a target-local entry supplies it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BinaryTargetIdentity {
    /// Cargo binary target name.
    pub name: String,
}

/// A stable source position without source contents.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceLocation {
    /// Source path as reported by the declaring target.
    pub file: String,
    /// One-based source line when known.
    pub line: Option<u32>,
    /// One-based source column when known.
    pub column: Option<u32>,
    /// Canonical Rust path of the declared item (`my_app::http::Users::list`), when known.
    ///
    /// Lets an IDE resolve the exact symbol (and its type information) even when `file` and
    /// `line` drift from the current buffer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_path: Option<String>,
}

/// Application and selected Cargo target identity.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DocumentIdentity {
    /// Configured application name.
    pub application: String,
    /// Selected Cargo package, when supplied by generated target metadata.
    pub package: Option<PackageIdentity>,
    /// Selected binary target, when supplied by generated target metadata.
    pub binary: Option<BinaryTargetIdentity>,
    /// Application declaration source, when supplied by generated target metadata.
    pub source: Option<SourceLocation>,
    /// Absolute Cargo workspace root that relative [`SourceLocation::file`] paths of workspace
    /// members resolve against, when supplied by the Cargo-side invoker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
}

/// Package and binary identity supplied by the Cargo-side probe invoker.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeTargetIdentity {
    package: PackageIdentity,
    binary: BinaryTargetIdentity,
}

impl ProbeTargetIdentity {
    /// Creates and validates one explicitly selected Cargo package and binary target.
    pub fn new(
        package: PackageIdentity,
        binary: BinaryTargetIdentity,
    ) -> Result<Self, IdentityValidationError> {
        let identity = Self { package, binary };

        identity.validate()?;

        Ok(identity)
    }

    /// Returns the selected Cargo package.
    pub const fn package(&self) -> &PackageIdentity {
        &self.package
    }

    /// Returns the selected Cargo binary target.
    pub const fn binary(&self) -> &BinaryTargetIdentity {
        &self.binary
    }

    /// Combines invoker-owned target identity with declaration-owned application identity.
    pub fn document_identity(
        &self,
        application: impl Into<String>,
        source: SourceLocation,
    ) -> Result<DocumentIdentity, IdentityValidationError> {
        let identity = DocumentIdentity {
            application: application.into(),
            package: Some(self.package.clone()),
            binary: Some(self.binary.clone()),
            source: Some(source),
            workspace_root: None,
        };

        validate_document_identity(&identity)?;

        Ok(identity)
    }

    fn validate(&self) -> Result<(), IdentityValidationError> {
        validate_package_identity(&self.package)?;

        if is_blank(&self.binary.name) {
            return Err(IdentityValidationError::MissingBinary);
        }

        Ok(())
    }
}

/// A selected target or generated application declaration has incomplete identity.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum IdentityValidationError {
    /// The generated application name is empty.
    #[error("tooling probe identity has no application name")]
    MissingApplication,
    /// The Cargo invoker did not supply a package name.
    #[error("tooling probe identity has no package name")]
    MissingPackage,
    /// A supplied package version is empty.
    #[error("tooling probe identity has an empty package version")]
    MissingPackageVersion,
    /// A supplied manifest path is empty or not absolute.
    #[error("tooling probe identity has an invalid package manifest path")]
    InvalidManifestPath,
    /// The Cargo invoker did not supply a binary target name.
    #[error("tooling probe identity has no binary target name")]
    MissingBinary,
    /// The generated declaration source has no file identity.
    #[error("tooling probe identity has no application declaration source")]
    MissingSource,
    /// A supplied source line is not one-based.
    #[error("tooling probe identity has an invalid application declaration line")]
    InvalidSourceLine,
    /// A supplied source column is not one-based.
    #[error("tooling probe identity has an invalid application declaration column")]
    InvalidSourceColumn,
}

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

fn validate_document_identity(identity: &DocumentIdentity) -> Result<(), IdentityValidationError> {
    if is_blank(&identity.application) {
        return Err(IdentityValidationError::MissingApplication);
    }

    validate_package_identity(
        identity
            .package
            .as_ref()
            .ok_or(IdentityValidationError::MissingPackage)?,
    )?;

    match &identity.binary {
        Some(binary) if is_blank(&binary.name) => {
            return Err(IdentityValidationError::MissingBinary);
        }
        Some(_) => {}
        None => return Err(IdentityValidationError::MissingBinary),
    }

    validate_source_location(
        identity
            .source
            .as_ref()
            .ok_or(IdentityValidationError::MissingSource)?,
    )?;

    Ok(())
}

fn validate_package_identity(package: &PackageIdentity) -> Result<(), IdentityValidationError> {
    if is_blank(&package.name) {
        return Err(IdentityValidationError::MissingPackage);
    }

    if package
        .version
        .as_ref()
        .is_some_and(|version| is_blank(version))
    {
        return Err(IdentityValidationError::MissingPackageVersion);
    }

    if package
        .manifest_path
        .as_ref()
        .is_some_and(|path| is_blank(path) || !is_portable_absolute_path(path))
    {
        return Err(IdentityValidationError::InvalidManifestPath);
    }

    Ok(())
}

fn is_portable_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let unix = bytes.first() == Some(&b'/');
    let unc = bytes.starts_with(b"\\\\");
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | 92);

    unix || unc || drive
}

fn validate_source_location(source: &SourceLocation) -> Result<(), IdentityValidationError> {
    if is_blank(&source.file) {
        return Err(IdentityValidationError::MissingSource);
    }

    if source.line == Some(0) {
        return Err(IdentityValidationError::InvalidSourceLine);
    }

    if source.column == Some(0) {
        return Err(IdentityValidationError::InvalidSourceColumn);
    }

    Ok(())
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

fn validate_tooling_document_identity(identity: &DocumentIdentity) -> Result<(), ValidationError> {
    if is_blank(&identity.application) {
        return Err(ValidationError::MissingApplicationIdentity);
    }

    if identity
        .package
        .as_ref()
        .is_some_and(|package| validate_package_identity(package).is_err())
        || identity
            .binary
            .as_ref()
            .is_some_and(|binary| is_blank(&binary.name))
        || identity
            .source
            .as_ref()
            .is_some_and(|source| validate_source_location(source).is_err())
    {
        return Err(ValidationError::InvalidDocumentIdentity);
    }

    Ok(())
}

/// Provenance shared by resources and diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    /// Stable identity of the owning resource.
    pub owner: Option<String>,
    /// Semantic origin such as `protocol-default` or `application-configuration`.
    pub origin: Option<String>,
    /// Origin-local declaration order, when meaningful.
    pub ordinal: Option<u32>,
    /// Declaration source, when available.
    pub source: Option<SourceLocation>,
}

/// A namespaced opaque extension payload with its own additive schema version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Facet {
    /// Facet-local schema version.
    pub schema_version: u16,
    /// Opaque JSON interpreted only by the facet owner.
    pub value: Value,
}

/// Generic resource categories understood without protocol knowledge.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ResourceKind {
    /// A diagnostic-only identity whose generic category is not known.
    Unknown,
    /// The configured application.
    Application,
    /// The selected protocol definition.
    Protocol,
    /// An effective plugin implementation.
    Plugin,
    /// A component descriptor.
    Component,
    /// A trait provider descriptor.
    Provider,
    /// A safe config type/path binding.
    ConfigBinding,
    /// A hook descriptor.
    Hook,
    /// A lifecycle stage.
    Lifecycle,
    /// A prepared scope boundary.
    Scope,
    /// A stable Rust type name used as a graph endpoint.
    Type,
    /// A plugin contribution decision.
    Contribution,
    /// A framework, application, protocol, or plugin contribution owner.
    Contributor,
    /// A plugin capability slot, including an unoccupied relation endpoint.
    PluginSlot,
    /// One node of a component's condition expression.
    Condition,
}

/// Classifies a diagnostic resource identity by its canonical generic prefix.
///
/// Producers may override this classification in [`ProbeFailure::resource_kinds`] when an
/// owner-qualified identity has more specific semantics.
pub fn diagnostic_resource_kind(id: &str) -> ResourceKind {
    let prefix = id.split_once(':').map_or(id, |(prefix, _)| prefix);

    match prefix {
        "application" => ResourceKind::Application,
        "protocol" => ResourceKind::Protocol,
        "plugin" => ResourceKind::Plugin,
        "component" => ResourceKind::Component,
        "provider" => ResourceKind::Provider,
        "config-binding" => ResourceKind::ConfigBinding,
        "hook" => ResourceKind::Hook,
        "lifecycle" => ResourceKind::Lifecycle,
        "scope" => ResourceKind::Scope,
        "type" => ResourceKind::Type,
        "contribution" => ResourceKind::Contribution,
        "plugin-slot" => ResourceKind::PluginSlot,
        "condition" => ResourceKind::Condition,
        "framework" => ResourceKind::Contributor,
        _ => ResourceKind::Unknown,
    }
}

/// Declarative owner-provided presentation for one resource.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceDisplay {
    /// Optional compact human-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional owner-defined visual category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Optional one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Deterministically keyed human-facing facts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

impl ResourceDisplay {
    /// Returns whether this declaration contains no presentation information.
    pub fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.group.is_none()
            && self.summary.is_none()
            && self.details.is_empty()
    }
}

/// One protocol-neutral inspectable resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Resource {
    /// Stable document-local identity.
    pub id: String,
    /// Generic resource category.
    pub kind: ResourceKind,
    /// Human-readable name.
    pub name: String,
    /// Declarative human presentation supplied by the resource owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ResourceDisplay>,
    /// Stable declaration provenance, when available.
    #[serde(default)]
    pub provenance: Option<Provenance>,
    /// Deterministically ordered scalar metadata.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Namespaced opaque extension data.
    #[serde(default)]
    pub facets: BTreeMap<String, Facet>,
}

impl Default for Resource {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: ResourceKind::Application,
            name: String::new(),
            display: None,
            provenance: None,
            labels: BTreeMap::new(),
            facets: BTreeMap::new(),
        }
    }
}

/// Typed generic relationship semantics.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum RelationshipKind {
    /// A resource requires another resource.
    DependsOn,
    /// A provider or component exposes a type.
    Provides,
    /// A config binding selects a type and safe property path.
    Binds,
    /// A hook belongs to a component or lifecycle stage.
    Hooks,
    /// A parent scope opens a child scope.
    OpensScope,
    /// A protocol, plugin, or owner-scoped contribution contains another contribution resource.
    Contains,
    /// A plugin emitted a retained contribution.
    Contributes,
    /// One plugin displaced another implementation.
    Replaces,
    /// A composition directive disabled a plugin implementation.
    Suppresses,
    /// The source precedes the target.
    OrdersBefore,
    /// The source follows the target.
    OrdersAfter,
    /// The source validated the target.
    Validates,
    /// The source cannot coexist with the target.
    Conflicts,
    /// A condition decides whether its target component, or parent condition, is eligible.
    Gates,
}

/// One directed relationship between resources.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Relationship {
    /// Relationship semantics.
    pub kind: RelationshipKind,
    /// Stable source resource identity.
    pub from: String,
    /// Stable target resource identity.
    pub to: String,
    /// Deterministically ordered scalar edge metadata.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Default for Relationship {
    fn default() -> Self {
        Self {
            kind: RelationshipKind::DependsOn,
            from: String::new(),
            to: String::new(),
            labels: BTreeMap::new(),
        }
    }
}

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

/// Complete typed metadata for the effective generated command-line parser.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliMetadata {
    /// Root command of the effective parser.
    pub root: CliCommand,
    /// Canonical command identity selected when no command is supplied.
    #[serde(default)]
    pub default_command: Option<String>,
    /// Effective plugin CLI providers referenced by parser element ownership.
    #[serde(default)]
    pub providers: Vec<CliProvider>,
}

/// One effective plugin CLI provider and its stable provenance.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliProvider {
    /// Stable document-local provider identity.
    pub id: String,
    /// Stable contributor resource identity.
    pub contributor: String,
    /// Contributor-local contribution identity.
    pub contribution: String,
    /// Parser-facing provider category.
    pub kind: CliProviderKind,
}

/// Borrowed contributor and owner-local contribution parsed from a canonical tooling identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContributionIdentity<'a> {
    contributor: &'a str,
    contribution: &'a str,
}

impl<'a> ContributionIdentity<'a> {
    /// Returns the canonical contributor resource identity.
    pub const fn contributor(self) -> &'a str {
        self.contributor
    }

    /// Returns the contributor-local contribution identity.
    pub const fn contribution(self) -> &'a str {
        self.contribution
    }
}

/// Constructs the canonical document-local identity for one contribution resource.
pub fn contribution_id(contributor: &str, contribution: &str) -> String {
    format!("contribution:{contributor}:{contribution}")
}

/// Parses a canonical contribution resource identity.
pub fn parse_contribution_id(id: &str) -> Option<ContributionIdentity<'_>> {
    parse_contribution_identity(id, "contribution:")
}

/// Constructs the canonical identity for one effective CLI provider.
pub fn cli_provider_id(contributor: &str, contribution: &str) -> String {
    format!("cli-provider:{contributor}:{contribution}")
}

/// Parses a canonical effective CLI-provider identity.
pub fn parse_cli_provider_id(id: &str) -> Option<ContributionIdentity<'_>> {
    parse_contribution_identity(id, "cli-provider:")
}

fn parse_contribution_identity<'a>(id: &'a str, prefix: &str) -> Option<ContributionIdentity<'a>> {
    let (contributor, contribution) = id.strip_prefix(prefix)?.rsplit_once(':')?;

    if is_blank(contributor) || is_blank(contribution) {
        return None;
    }

    Some(ContributionIdentity {
        contributor,
        contribution,
    })
}

/// Parser-facing category of a plugin CLI provider.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CliProviderKind {
    /// A flattened global argument group.
    #[default]
    Args,
    /// One named command.
    Command,
    /// A flattened native command set.
    CommandSet,
}

/// Stable ownership of one parser command or argument.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum CliOwner {
    /// A framework-reserved parser facility.
    Framework,
    /// Shape declared by the generated application.
    #[default]
    Application,
    /// Shape declared by an effective plugin provider.
    Plugin {
        /// Stable provider identity in [`CliMetadata::providers`].
        provider: String,
    },
}

/// One command in the effective parser tree.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliCommand {
    /// Optional canonical framework command identity, independent of its displayed name.
    #[serde(default)]
    pub id: Option<String>,
    /// Parser-facing command name.
    pub name: String,
    /// Hidden command aliases.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Help-visible command aliases.
    #[serde(default)]
    pub visible_aliases: Vec<String>,
    /// Primary short command flag.
    #[serde(default)]
    pub short_flag: Option<char>,
    /// Hidden short command flag aliases.
    #[serde(default)]
    pub short_flag_aliases: Vec<char>,
    /// Help-visible short command flag aliases.
    #[serde(default)]
    pub visible_short_flag_aliases: Vec<char>,
    /// Primary long command flag.
    #[serde(default)]
    pub long_flag: Option<String>,
    /// Hidden long command flag aliases.
    #[serde(default)]
    pub long_flag_aliases: Vec<String>,
    /// Help-visible long command flag aliases.
    #[serde(default)]
    pub visible_long_flag_aliases: Vec<String>,
    /// Short help text.
    #[serde(default)]
    pub help: Option<String>,
    /// Long help text.
    #[serde(default)]
    pub long_help: Option<String>,
    /// Whether normal help output hides this command.
    #[serde(default)]
    pub hidden: bool,
    /// Stable declaration owner.
    pub owner: CliOwner,
    /// Arguments declared at this command.
    #[serde(default)]
    pub arguments: Vec<CliArgument>,
    /// Direct nested commands.
    #[serde(default)]
    pub commands: Vec<CliCommand>,
}

/// One argument in the effective parser tree.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliArgument {
    /// Stable Clap argument ID and canonical generated framework argument identity.
    pub id: String,
    /// Primary long option.
    #[serde(default)]
    pub long: Option<String>,
    /// Primary short option.
    #[serde(default)]
    pub short: Option<char>,
    /// One-based positional index.
    #[serde(default)]
    pub index: Option<usize>,
    /// Hidden long aliases.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Help-visible long aliases.
    #[serde(default)]
    pub visible_aliases: Vec<String>,
    /// Hidden short aliases.
    #[serde(default)]
    pub short_aliases: Vec<char>,
    /// Help-visible short aliases.
    #[serde(default)]
    pub visible_short_aliases: Vec<char>,
    /// Short help text.
    #[serde(default)]
    pub help: Option<String>,
    /// Long help text.
    #[serde(default)]
    pub long_help: Option<String>,
    /// Ordered value placeholders.
    #[serde(default)]
    pub value_names: Vec<String>,
    /// Effective parser defaults after Clap renders literal or typed declarations.
    ///
    /// Non-Unicode OS values use the reversible `os-bytes:<hex>` representation.
    #[serde(default)]
    pub default_values: Vec<String>,
    /// Whether the argument is required.
    #[serde(default)]
    pub required: bool,
    /// Whether the argument propagates to nested commands.
    #[serde(default)]
    pub global: bool,
    /// Whether normal help output hides the argument.
    #[serde(default)]
    pub hidden: bool,
    /// Typed values-per-occurrence and repetition semantics.
    pub cardinality: CliCardinality,
    /// Stable declaration owner.
    pub owner: CliOwner,
}

/// Typed cardinality of one argument occurrence.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliCardinality {
    /// Minimum values accepted by one occurrence.
    pub min_values: usize,
    /// Maximum values accepted by one occurrence, or no bound.
    pub max_values: Option<usize>,
    /// Whether the option may occur repeatedly.
    pub repeatable: bool,
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self {
            valid: true,
            diagnostic_codes: Vec::new(),
        }
    }
}

/// The canonical protocol-neutral application inspection document.
///
/// Within one major version, new fields may be added only with defaults and existing field
/// semantics remain unchanged. Breaking field or enum changes require a new major version.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolingDocument {
    /// Document compatibility version.
    pub schema: Version,
    /// Upwell framework version that produced the document.
    pub framework_version: String,
    /// Application/package/binary/source identity.
    pub identity: DocumentIdentity,
    /// Stable selected protocol identity.
    pub protocol: String,
    /// Complete effective CLI parser metadata when the target has CLI support enabled.
    #[serde(default)]
    pub cli: Option<CliMetadata>,
    /// Generic resources in canonical order.
    #[serde(default)]
    pub resources: Vec<Resource>,
    /// Generic relationships in canonical order.
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    /// Typed diagnostics in canonical order.
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    /// Validation summary derived from diagnostics.
    #[serde(default)]
    pub validation: ValidationResult,
    /// Document-level namespaced opaque facets.
    #[serde(default)]
    pub facets: BTreeMap<String, Facet>,
    /// The plan of a proposed config change, when the invoker requested a preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_preview: Option<TransitionPreview>,
}

impl ToolingDocument {
    /// Creates an empty successful document for an application and selected protocol.
    pub fn new(
        framework_version: impl Into<String>,
        identity: DocumentIdentity,
        protocol: impl Into<String>,
    ) -> Self {
        Self {
            framework_version: framework_version.into(),
            identity,
            protocol: protocol.into(),
            ..Self::default()
        }
    }

    /// Sorts every order-insensitive collection and refreshes validation summary data.
    pub fn canonicalize(&mut self) {
        if let Some(cli) = &mut self.cli {
            canonicalize_cli(cli);
        }

        canonicalize_diagnostics(&mut self.diagnostics);

        self.resources.sort_by(|left, right| left.id.cmp(&right.id));
        self.relationships.sort_by(|left, right| {
            (&left.from, &left.kind, &left.to, &left.labels).cmp(&(
                &right.from,
                &right.kind,
                &right.to,
                &right.labels,
            ))
        });
        self.diagnostics.sort_by(|left, right| {
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

        self.validation = validation_result(&self.diagnostics);
    }

    /// Whether this document's schema satisfies a consumer's semantic-version requirement.
    pub fn schema_matches(&self, requirement: &VersionReq) -> bool {
        requirement.matches(&self.schema)
    }

    /// Validates schema and referential invariants without changing the document.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !schema_requirement().matches(&self.schema) {
            return Err(ValidationError::IncompatibleSchema {
                actual: self.schema.clone(),
                required: schema_requirement(),
            });
        }

        validate_tooling_document_identity(&self.identity)?;

        if is_blank(&self.framework_version) {
            return Err(ValidationError::MissingFrameworkVersion);
        }

        if is_blank(&self.protocol) {
            return Err(ValidationError::MissingProtocolIdentity);
        }

        validate_facets(&self.facets)?;

        let mut resource_ids = BTreeSet::new();
        let mut resources = BTreeMap::new();

        for resource in &self.resources {
            if is_blank(&resource.id) {
                return Err(ValidationError::EmptyResourceId);
            }

            if is_blank(&resource.name) {
                return Err(ValidationError::EmptyResourceName {
                    id: resource.id.clone(),
                });
            }

            if !resource_ids.insert(resource.id.as_str()) {
                return Err(ValidationError::DuplicateResource {
                    id: resource.id.clone(),
                });
            }

            resources.insert(resource.id.as_str(), resource);
            validate_resource_display(resource)?;
            validate_resource_facets(resource)?;
        }

        for resource in &self.resources {
            validate_provenance(resource, &resources)?;
        }

        if let Some(cli) = &self.cli {
            validate_cli(cli, &resources)?;
        }

        let mut relationships = BTreeSet::new();

        for relationship in &self.relationships {
            let from = relationship_resource(&resources, &relationship.from)?;
            let to = relationship_resource(&resources, &relationship.to)?;
            let identity = (
                relationship.kind.clone(),
                relationship.from.as_str(),
                relationship.to.as_str(),
            );

            if !relationships.insert(identity) {
                return Err(ValidationError::DuplicateRelationship {
                    kind: relationship.kind.clone(),
                    from: relationship.from.clone(),
                    to: relationship.to.clone(),
                });
            }

            validate_relationship_kinds(relationship, from, to)?;
        }

        for diagnostic in &self.diagnostics {
            validate_diagnostic(diagnostic)?;

            for resource in &diagnostic.resources {
                if !resources.contains_key(resource.as_str()) {
                    return Err(ValidationError::UnknownDiagnosticResource {
                        id: resource.clone(),
                    });
                }
            }
        }

        let expected = validation_result(&self.diagnostics);

        if self.validation != expected {
            return Err(ValidationError::InconsistentValidationResult {
                actual: self.validation.clone(),
                expected,
            });
        }

        Ok(())
    }

    /// Emits compact deterministic JSON after canonicalization and validation.
    pub fn to_canonical_json(&self) -> Result<String, EmitError> {
        let mut document = self.clone();

        document.canonicalize();
        document.validate()?;

        Ok(serde_json::to_string(&document)?)
    }
}

impl Default for ToolingDocument {
    fn default() -> Self {
        Self {
            schema: TOOLING_SCHEMA_VERSION,
            framework_version: String::new(),
            identity: DocumentIdentity::default(),
            protocol: String::new(),
            cli: None,
            resources: Vec::new(),
            relationships: Vec::new(),
            diagnostics: Vec::new(),
            validation: ValidationResult::default(),
            facets: BTreeMap::new(),
            transition_preview: None,
        }
    }
}

/// A structural schema validation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    /// The document belongs to an incompatible tooling-schema release.
    #[error("incompatible tooling schema {actual}; expected {required}")]
    IncompatibleSchema {
        /// Encountered schema version.
        actual: Version,
        /// Compatible schema releases.
        required: VersionReq,
    },
    /// The application identity is absent.
    #[error("tooling document has no application identity")]
    MissingApplicationIdentity,
    /// Optional package, binary, or source identity is present but empty.
    #[error("tooling document contains an incomplete target identity")]
    InvalidDocumentIdentity,
    /// The framework version is absent.
    #[error("tooling document has no framework version")]
    MissingFrameworkVersion,
    /// The selected protocol identity is absent.
    #[error("tooling document has no protocol identity")]
    MissingProtocolIdentity,
    /// A CLI provider has no stable identity.
    #[error("tooling CLI metadata contains an empty provider identity")]
    EmptyCliProviderId,
    /// A CLI provider has no contributor identity.
    #[error("tooling CLI provider '{id}' has no contributor")]
    EmptyCliProviderContributor {
        /// Provider identity.
        id: String,
    },
    /// A CLI provider has no contributor-local contribution identity.
    #[error("tooling CLI provider '{id}' has no contribution")]
    EmptyCliProviderContribution {
        /// Provider identity.
        id: String,
    },
    /// A CLI provider identity does not match its contributor and contribution.
    #[error(
        "tooling CLI provider '{id}' does not match contributor '{contributor}' and contribution '{contribution}'"
    )]
    InvalidCliProviderIdentity {
        /// Provider identity.
        id: String,
        /// Contributor identity.
        contributor: String,
        /// Contributor-local contribution identity.
        contribution: String,
    },
    /// A CLI provider contributor does not exist or cannot own contributions.
    #[error("tooling CLI provider '{id}' has invalid contributor '{contributor}'")]
    InvalidCliProviderContributor {
        /// Provider identity.
        id: String,
        /// Invalid contributor identity.
        contributor: String,
    },
    /// Two CLI providers use the same stable identity.
    #[error("tooling CLI metadata contains duplicate provider '{id}'")]
    DuplicateCliProvider {
        /// Duplicated provider identity.
        id: String,
    },
    /// A CLI command has no canonical name.
    #[error("tooling CLI metadata contains a command with no name at '{path}'")]
    EmptyCliCommandName {
        /// Parent command path.
        path: String,
    },
    /// A canonical CLI command identity is present but blank.
    #[error("tooling CLI metadata contains an empty canonical command ID at '{path}'")]
    EmptyCliCommandId {
        /// Command path.
        path: String,
    },
    /// Two CLI commands use the same canonical identity.
    #[error("tooling CLI metadata contains duplicate canonical command ID '{id}'")]
    DuplicateCliCommandId {
        /// Duplicated canonical identity.
        id: String,
    },
    /// The default command does not identify a canonical command in the parser.
    #[error("tooling CLI metadata refers to unknown default command '{id}'")]
    UnknownDefaultCliCommand {
        /// Missing canonical command identity.
        id: String,
    },
    /// Two sibling CLI commands use the same canonical name.
    #[error("tooling CLI metadata contains duplicate command '{name}' at '{path}'")]
    DuplicateCliCommand {
        /// Parent command path.
        path: String,
        /// Duplicated command name.
        name: String,
    },
    /// A parser-facing command or option name is blank.
    #[error("tooling CLI metadata contains an empty {kind} at '{path}'")]
    EmptyCliParserName {
        /// Containing command path.
        path: String,
        /// Parser namespace category.
        kind: &'static str,
    },
    /// Two parser elements claim the same long option namespace entry.
    #[error("tooling CLI metadata contains duplicate long option '--{name}' at '{path}'")]
    DuplicateCliLongOption {
        /// Containing command path.
        path: String,
        /// Duplicated long option.
        name: String,
    },
    /// Two parser elements claim the same short option namespace entry.
    #[error("tooling CLI metadata contains duplicate short option '-{name}' at '{path}'")]
    DuplicateCliShortOption {
        /// Containing command path.
        path: String,
        /// Duplicated short option.
        name: char,
    },
    /// A CLI argument has no stable ID.
    #[error("tooling CLI metadata contains an argument with no ID at '{path}'")]
    EmptyCliArgumentId {
        /// Containing command path.
        path: String,
    },
    /// Two CLI arguments use the same ID in one command.
    #[error("tooling CLI metadata contains duplicate argument '{id}' at '{path}'")]
    DuplicateCliArgument {
        /// Containing command path.
        path: String,
        /// Duplicated argument ID.
        id: String,
    },
    /// Two positional arguments use the same index in one command.
    #[error("tooling CLI metadata contains duplicate positional index {index} at '{path}'")]
    DuplicateCliPosition {
        /// Containing command path.
        path: String,
        /// Duplicated one-based position.
        index: usize,
    },
    /// A positional argument uses zero instead of a one-based index.
    #[error("tooling CLI argument '{id}' at '{path}' uses invalid positional index zero")]
    InvalidCliPosition {
        /// Containing command path.
        path: String,
        /// Argument ID.
        id: String,
    },
    /// An argument cardinality has a maximum below its minimum.
    #[error("tooling CLI argument '{id}' at '{path}' has invalid cardinality {min}..={max}")]
    InvalidCliCardinality {
        /// Containing command path.
        path: String,
        /// Argument ID.
        id: String,
        /// Minimum values.
        min: usize,
        /// Invalid maximum values.
        max: usize,
    },
    /// A parser element refers to an undeclared plugin provider.
    #[error("tooling CLI metadata refers to unknown provider '{id}' at '{path}'")]
    UnknownCliProvider {
        /// Command or argument path containing the reference.
        path: String,
        /// Missing provider identity.
        id: String,
    },
    /// A resource has no stable identity.
    #[error("tooling document contains an empty resource identity")]
    EmptyResourceId,
    /// A resource has no display name.
    #[error("tooling resource '{id}' has no name")]
    EmptyResourceName {
        /// Resource identity.
        id: String,
    },
    /// A declarative resource display contains no presentation information.
    #[error("tooling resource '{id}' has an empty display declaration")]
    EmptyResourceDisplay {
        /// Resource identity.
        id: String,
    },
    /// A declarative resource display contains blank text.
    #[error("tooling resource '{id}' has blank display text")]
    BlankResourceDisplayText {
        /// Resource identity.
        id: String,
    },
    /// A declarative resource display contains an oversized text value.
    #[error("tooling resource '{id}' has display text exceeding 16384 bytes")]
    ResourceDisplayTextTooLarge {
        /// Resource identity.
        id: String,
    },
    /// Two resources use the same stable identity.
    #[error("tooling document contains duplicate resource '{id}'")]
    DuplicateResource {
        /// Duplicated identity.
        id: String,
    },
    /// A relationship endpoint does not exist.
    #[error("relationship refers to unknown resource '{id}'")]
    UnknownRelationshipResource {
        /// Missing identity.
        id: String,
    },
    /// Two relationships repeat the same typed endpoints.
    #[error("duplicate {kind:?} relationship from '{from}' to '{to}'")]
    DuplicateRelationship {
        /// Relationship semantics.
        kind: RelationshipKind,
        /// Source identity.
        from: String,
        /// Target identity.
        to: String,
    },
    /// A relationship connects resource kinds incompatible with its semantics.
    #[error("{kind:?} relationship cannot connect {from_kind:?} '{from}' to {to_kind:?} '{to}'")]
    InvalidRelationshipKinds {
        /// Relationship semantics.
        kind: RelationshipKind,
        /// Source identity.
        from: String,
        /// Source kind.
        from_kind: ResourceKind,
        /// Target identity.
        to: String,
        /// Target kind.
        to_kind: ResourceKind,
    },
    /// A diagnostic resource reference does not exist.
    #[error("diagnostic refers to unknown resource '{id}'")]
    UnknownDiagnosticResource {
        /// Missing identity.
        id: String,
    },
    /// A diagnostic is structurally invalid.
    #[error(transparent)]
    Diagnostic(#[from] DiagnosticValidationError),
    /// A resource provenance owner does not exist.
    #[error("resource '{id}' provenance refers to unknown owner '{owner}'")]
    UnknownProvenanceOwner {
        /// Owned resource identity.
        id: String,
        /// Missing owner identity.
        owner: String,
    },
    /// A resource provenance owner cannot own tooling resources.
    #[error("resource '{id}' provenance owner '{owner}' has invalid kind {kind:?}")]
    InvalidProvenanceOwnerKind {
        /// Owned resource identity.
        id: String,
        /// Owner identity.
        owner: String,
        /// Invalid owner resource kind.
        kind: ResourceKind,
    },
    /// A resource provenance source has no file identity.
    #[error("resource '{id}' provenance contains an empty source file identity")]
    EmptyProvenanceSource {
        /// Resource identity.
        id: String,
    },
    /// A resource provenance source position is not one-based.
    #[error("resource '{id}' provenance contains invalid source coordinates")]
    InvalidProvenanceSource {
        /// Resource identity.
        id: String,
    },
    /// A non-owner resource claims to own itself.
    #[error("resource '{id}' has self-inconsistent provenance ownership")]
    SelfOwnedResource {
        /// Resource identity.
        id: String,
    },
    /// A facet identity is not namespaced.
    #[error("facet identity '{id}' is not namespaced")]
    InvalidFacetId {
        /// Invalid facet identity.
        id: String,
    },
    /// A facet has no positive local schema version.
    #[error("facet '{id}' has schema version zero")]
    InvalidFacetVersion {
        /// Invalid facet identity.
        id: String,
    },
    /// A resource facet is not qualified by its owning resource identity.
    #[error("facet '{id}' is not owned by resource '{owner}'")]
    InvalidFacetOwner {
        /// Invalid facet identity.
        id: String,
        /// Resource carrying the facet.
        owner: String,
    },
    /// The serialized validation summary disagrees with the diagnostics.
    #[error("tooling validation summary is inconsistent with diagnostics")]
    InconsistentValidationResult {
        /// Summary present in the document.
        actual: ValidationResult,
        /// Summary computed from the document diagnostics.
        expected: ValidationResult,
    },
}

/// A typed failure while producing canonical JSON.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EmitError {
    /// Document validation failed.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// JSON serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

fn validate_facets(facets: &BTreeMap<String, Facet>) -> Result<(), ValidationError> {
    for (id, facet) in facets {
        if !is_namespaced(id) {
            return Err(ValidationError::InvalidFacetId { id: id.clone() });
        }

        if facet.schema_version == 0 {
            return Err(ValidationError::InvalidFacetVersion { id: id.clone() });
        }
    }

    Ok(())
}

fn validate_diagnostic(diagnostic: &Diagnostic) -> Result<(), DiagnosticValidationError> {
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

fn canonicalize_diagnostics(diagnostics: &mut [Diagnostic]) {
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

fn validate_provenance(
    resource: &Resource,
    resources: &BTreeMap<&str, &Resource>,
) -> Result<(), ValidationError> {
    let Some(provenance) = resource.provenance.as_ref() else {
        return Ok(());
    };
    if let Some(source) = provenance.source.as_ref() {
        match validate_source_location(source) {
            Ok(()) => {}
            Err(IdentityValidationError::MissingSource) => {
                return Err(ValidationError::EmptyProvenanceSource {
                    id: resource.id.clone(),
                });
            }
            Err(_) => {
                return Err(ValidationError::InvalidProvenanceSource {
                    id: resource.id.clone(),
                });
            }
        }
    }

    let Some(owner) = provenance.owner.as_deref() else {
        return Ok(());
    };
    let Some(owner_resource) = resources.get(owner) else {
        return Err(ValidationError::UnknownProvenanceOwner {
            id: resource.id.clone(),
            owner: owner.to_string(),
        });
    };
    let owner_kind = &owner_resource.kind;
    let allowed = matches!(
        owner_kind,
        ResourceKind::Application
            | ResourceKind::Protocol
            | ResourceKind::Plugin
            | ResourceKind::Contributor
    );

    if !allowed {
        return Err(ValidationError::InvalidProvenanceOwnerKind {
            id: resource.id.clone(),
            owner: owner.to_string(),
            kind: owner_kind.clone(),
        });
    }

    if owner == resource.id && !allowed_provenance_self_owner(&resource.kind) {
        return Err(ValidationError::SelfOwnedResource {
            id: resource.id.clone(),
        });
    }

    Ok(())
}

fn validate_resource_display(resource: &Resource) -> Result<(), ValidationError> {
    const MAX_TEXT_BYTES: usize = 16 * 1024;

    let Some(display) = &resource.display else {
        return Ok(());
    };

    if display.is_empty() {
        return Err(ValidationError::EmptyResourceDisplay {
            id: resource.id.clone(),
        });
    }

    let optional = display
        .label
        .iter()
        .chain(&display.group)
        .chain(&display.summary);

    if optional.clone().any(|value| is_blank(value))
        || display
            .details
            .iter()
            .any(|(name, value)| is_blank(name) || is_blank(value))
    {
        return Err(ValidationError::BlankResourceDisplayText {
            id: resource.id.clone(),
        });
    }

    if optional
        .chain(display.details.keys())
        .chain(display.details.values())
        .any(|value| value.len() > MAX_TEXT_BYTES)
    {
        return Err(ValidationError::ResourceDisplayTextTooLarge {
            id: resource.id.clone(),
        });
    }

    Ok(())
}

fn allowed_provenance_self_owner(kind: &ResourceKind) -> bool {
    matches!(
        kind,
        ResourceKind::Application
            | ResourceKind::Protocol
            | ResourceKind::Plugin
            | ResourceKind::Contributor
    )
}

fn relationship_resource<'a>(
    resources: &'a BTreeMap<&str, &'a Resource>,
    id: &str,
) -> Result<&'a Resource, ValidationError> {
    resources
        .get(id)
        .copied()
        .ok_or_else(|| ValidationError::UnknownRelationshipResource { id: id.to_string() })
}

fn validate_relationship_kinds(
    relationship: &Relationship,
    from: &Resource,
    to: &Resource,
) -> Result<(), ValidationError> {
    let valid = match relationship.kind {
        RelationshipKind::OpensScope => {
            from.kind == ResourceKind::Scope && to.kind == ResourceKind::Scope
        }
        RelationshipKind::Provides => matches!(
            (&from.kind, &to.kind),
            (
                ResourceKind::Component,
                ResourceKind::Type | ResourceKind::Provider
            ) | (ResourceKind::Provider, ResourceKind::Type)
                | (ResourceKind::Plugin, ResourceKind::PluginSlot)
        ),
        RelationshipKind::Binds => matches!(
            (&from.kind, &to.kind),
            (ResourceKind::ConfigBinding, ResourceKind::Type)
        ),
        RelationshipKind::Replaces => {
            from.kind == ResourceKind::Plugin && to.kind == ResourceKind::Plugin
        }
        RelationshipKind::Suppresses => {
            from.kind == ResourceKind::Contribution && to.kind == ResourceKind::Plugin
        }
        RelationshipKind::Hooks => matches!(
            (&from.kind, &to.kind),
            (ResourceKind::Component, ResourceKind::Hook)
                | (ResourceKind::Hook, ResourceKind::Lifecycle)
        ),
        RelationshipKind::Gates => matches!(
            (&from.kind, &to.kind),
            (
                ResourceKind::Condition,
                ResourceKind::Component | ResourceKind::Condition
            )
        ),
        RelationshipKind::Contains => valid_contains_relationship(&from.kind, &to.kind),
        RelationshipKind::Contributes => valid_contribution_relationship(&from.kind, &to.kind),
        _ => true,
    };

    if !valid {
        return Err(ValidationError::InvalidRelationshipKinds {
            kind: relationship.kind.clone(),
            from: relationship.from.clone(),
            from_kind: from.kind.clone(),
            to: relationship.to.clone(),
            to_kind: to.kind.clone(),
        });
    }

    Ok(())
}

fn valid_contains_relationship(from: &ResourceKind, to: &ResourceKind) -> bool {
    let owner = matches!(from, ResourceKind::Protocol | ResourceKind::Plugin);

    (owner || *from == ResourceKind::Contribution) && *to == ResourceKind::Contribution
}

fn valid_contribution_relationship(from: &ResourceKind, to: &ResourceKind) -> bool {
    let owner = matches!(from, ResourceKind::Plugin | ResourceKind::Contributor);
    let owned = *to == ResourceKind::Contribution;
    let projected_target = matches!(
        to,
        ResourceKind::Component | ResourceKind::Provider | ResourceKind::ConfigBinding
    );

    (owner && owned)
        || (*from == ResourceKind::Contribution
            && (*to == ResourceKind::Contribution || projected_target))
}

fn validate_resource_facets(resource: &Resource) -> Result<(), ValidationError> {
    validate_facets(&resource.facets)?;

    let owner_prefix = format!("{}/", resource.id);

    for id in resource.facets.keys() {
        if !id.starts_with(&owner_prefix) {
            return Err(ValidationError::InvalidFacetOwner {
                id: id.clone(),
                owner: resource.id.clone(),
            });
        }
    }

    Ok(())
}

fn canonicalize_cli(cli: &mut CliMetadata) {
    cli.providers.sort_by(|left, right| left.id.cmp(&right.id));
    canonicalize_cli_command(&mut cli.root);
}

fn canonicalize_cli_command(command: &mut CliCommand) {
    sort_dedup(&mut command.aliases);
    sort_dedup(&mut command.visible_aliases);
    sort_dedup(&mut command.short_flag_aliases);
    sort_dedup(&mut command.visible_short_flag_aliases);
    sort_dedup(&mut command.long_flag_aliases);
    sort_dedup(&mut command.visible_long_flag_aliases);

    for argument in &mut command.arguments {
        sort_dedup(&mut argument.aliases);
        sort_dedup(&mut argument.visible_aliases);
        sort_dedup(&mut argument.short_aliases);
        sort_dedup(&mut argument.visible_short_aliases);
    }

    for child in &mut command.commands {
        canonicalize_cli_command(child);
    }

    command.arguments.sort_by(|left, right| {
        (left.index.is_none(), left.index, &left.id).cmp(&(
            right.index.is_none(),
            right.index,
            &right.id,
        ))
    });
    command
        .commands
        .sort_by(|left, right| left.name.cmp(&right.name));
}

fn sort_dedup<T: Ord>(values: &mut Vec<T>) {
    values.sort();
    values.dedup();
}

fn validate_cli(
    cli: &CliMetadata,
    resources: &BTreeMap<&str, &Resource>,
) -> Result<(), ValidationError> {
    let mut providers = BTreeSet::new();
    let mut command_ids = BTreeSet::new();

    for provider in &cli.providers {
        if is_blank(&provider.id) {
            return Err(ValidationError::EmptyCliProviderId);
        }

        if is_blank(&provider.contributor) {
            return Err(ValidationError::EmptyCliProviderContributor {
                id: provider.id.clone(),
            });
        }

        if is_blank(&provider.contribution) {
            return Err(ValidationError::EmptyCliProviderContribution {
                id: provider.id.clone(),
            });
        }

        let identity = parse_cli_provider_id(&provider.id);

        if identity.is_none_or(|identity| {
            identity.contributor() != provider.contributor
                || identity.contribution() != provider.contribution
        }) {
            return Err(ValidationError::InvalidCliProviderIdentity {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
                contribution: provider.contribution.clone(),
            });
        }

        let Some(contributor) = resources.get(provider.contributor.as_str()) else {
            return Err(ValidationError::InvalidCliProviderContributor {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
            });
        };

        if !allowed_provenance_self_owner(&contributor.kind) {
            return Err(ValidationError::InvalidCliProviderContributor {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
            });
        }

        if !providers.insert(provider.id.as_str()) {
            return Err(ValidationError::DuplicateCliProvider {
                id: provider.id.clone(),
            });
        }
    }

    validate_cli_command(
        &cli.root,
        "",
        &providers,
        &CliOptionNamespace::default(),
        &mut command_ids,
    )?;

    if let Some(default_command) = &cli.default_command
        && (is_blank(default_command) || !command_ids.contains(default_command))
    {
        return Err(ValidationError::UnknownDefaultCliCommand {
            id: default_command.clone(),
        });
    }

    Ok(())
}

#[derive(Clone, Default)]
struct CliOptionNamespace {
    long: BTreeSet<String>,
    short: BTreeSet<char>,
}

fn validate_cli_command(
    command: &CliCommand,
    parent: &str,
    providers: &BTreeSet<&str>,
    inherited: &CliOptionNamespace,
    command_ids: &mut BTreeSet<String>,
) -> Result<(), ValidationError> {
    if is_blank(&command.name) {
        return Err(ValidationError::EmptyCliCommandName {
            path: parent.to_string(),
        });
    }

    let path = if parent.is_empty() {
        command.name.clone()
    } else {
        format!("{parent} {}", command.name)
    };
    let mut arguments = BTreeSet::new();
    let mut positions = BTreeSet::new();
    let mut commands = BTreeSet::new();
    let mut options = inherited.clone();
    let mut globals = inherited.clone();

    validate_cli_owner(&command.owner, &path, providers)?;

    if let Some(id) = &command.id {
        if is_blank(id) {
            return Err(ValidationError::EmptyCliCommandId { path });
        }

        if !command_ids.insert(id.clone()) {
            return Err(ValidationError::DuplicateCliCommandId { id: id.clone() });
        }
    }

    for argument in &command.arguments {
        if is_blank(&argument.id) {
            return Err(ValidationError::EmptyCliArgumentId { path });
        }

        if !arguments.insert(argument.id.as_str()) {
            return Err(ValidationError::DuplicateCliArgument {
                path,
                id: argument.id.clone(),
            });
        }

        if let Some(index) = argument.index {
            if index == 0 {
                return Err(ValidationError::InvalidCliPosition {
                    path,
                    id: argument.id.clone(),
                });
            }

            if !positions.insert(index) {
                return Err(ValidationError::DuplicateCliPosition { path, index });
            }
        }

        if let Some(max) = argument.cardinality.max_values
            && max < argument.cardinality.min_values
        {
            return Err(ValidationError::InvalidCliCardinality {
                path,
                id: argument.id.clone(),
                min: argument.cardinality.min_values,
                max,
            });
        }

        validate_cli_owner(
            &argument.owner,
            &format!("{path}::{}", argument.id),
            providers,
        )?;

        for long in argument
            .long
            .iter()
            .chain(&argument.aliases)
            .chain(&argument.visible_aliases)
        {
            insert_cli_long(&mut options, &path, long)?;

            if argument.global {
                insert_cli_long_if_absent(&mut globals, long);
            }
        }

        for short in argument
            .short
            .iter()
            .chain(&argument.short_aliases)
            .chain(&argument.visible_short_aliases)
        {
            insert_cli_short(&mut options, &path, *short)?;

            if argument.global {
                globals.short.insert(*short);
            }
        }
    }

    for child in &command.commands {
        for name in std::iter::once(&child.name)
            .chain(&child.aliases)
            .chain(&child.visible_aliases)
        {
            if is_blank(name) {
                return Err(ValidationError::EmptyCliParserName {
                    path,
                    kind: "command name or alias",
                });
            }

            if !commands.insert(name.as_str()) {
                return Err(ValidationError::DuplicateCliCommand {
                    path,
                    name: name.clone(),
                });
            }
        }

        for long in child
            .long_flag
            .iter()
            .chain(&child.long_flag_aliases)
            .chain(&child.visible_long_flag_aliases)
        {
            insert_cli_long(&mut options, &path, long)?;
        }

        for short in child
            .short_flag
            .iter()
            .chain(&child.short_flag_aliases)
            .chain(&child.visible_short_flag_aliases)
        {
            insert_cli_short(&mut options, &path, *short)?;
        }

        validate_cli_command(child, &path, providers, &globals, command_ids)?;
    }

    Ok(())
}

fn insert_cli_long(
    namespace: &mut CliOptionNamespace,
    path: &str,
    name: &str,
) -> Result<(), ValidationError> {
    if is_blank(name) {
        return Err(ValidationError::EmptyCliParserName {
            path: path.to_string(),
            kind: "long option",
        });
    }

    if !namespace.long.insert(name.to_string()) {
        return Err(ValidationError::DuplicateCliLongOption {
            path: path.to_string(),
            name: name.to_string(),
        });
    }

    Ok(())
}

fn insert_cli_long_if_absent(namespace: &mut CliOptionNamespace, name: &str) {
    namespace.long.insert(name.to_string());
}

fn insert_cli_short(
    namespace: &mut CliOptionNamespace,
    path: &str,
    name: char,
) -> Result<(), ValidationError> {
    if name.is_whitespace() {
        return Err(ValidationError::EmptyCliParserName {
            path: path.to_string(),
            kind: "short option",
        });
    }

    if !namespace.short.insert(name) {
        return Err(ValidationError::DuplicateCliShortOption {
            path: path.to_string(),
            name,
        });
    }

    Ok(())
}

fn validate_cli_owner(
    owner: &CliOwner,
    path: &str,
    providers: &BTreeSet<&str>,
) -> Result<(), ValidationError> {
    let CliOwner::Plugin { provider } = owner else {
        return Ok(());
    };

    if !providers.contains(provider.as_str()) {
        return Err(ValidationError::UnknownCliProvider {
            path: path.to_string(),
            id: provider.clone(),
        });
    }

    Ok(())
}

fn validation_result(diagnostics: &[Diagnostic]) -> ValidationResult {
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

fn is_namespaced(value: &str) -> bool {
    let mut parts = value.split('/');

    matches!((parts.next(), parts.next()), (Some(namespace), Some(name)) if !namespace.is_empty() && !name.is_empty())
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests;
