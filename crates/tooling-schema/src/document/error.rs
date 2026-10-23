//! Tooling document validation errors.

use semver::{Version, VersionReq};
use thiserror::Error;

use crate::{DiagnosticValidationError, RelationshipKind, ResourceKind, ValidationResult};

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
