//! Package, binary-target, and document identities and their validation.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::is_blank;

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

pub(crate) fn validate_document_identity(
    identity: &DocumentIdentity,
) -> Result<(), IdentityValidationError> {
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

pub(crate) fn validate_package_identity(
    package: &PackageIdentity,
) -> Result<(), IdentityValidationError> {
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

pub(crate) fn validate_source_location(
    source: &SourceLocation,
) -> Result<(), IdentityValidationError> {
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
