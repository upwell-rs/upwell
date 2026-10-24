//! The tooling document.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    CliMetadata, Diagnostic, DocumentIdentity, Facet, Relationship, Resource,
    TOOLING_SCHEMA_VERSION, TransitionPreview, ValidationResult, canonicalize_cli,
    canonicalize_diagnostics, is_blank, schema_requirement, validate_cli, validate_diagnostic,
    validation_result,
};

mod error;
mod validate;

pub use error::*;
pub(crate) use validate::*;

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
