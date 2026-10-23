use std::collections::BTreeMap;
use std::error::Error as _;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use futures::FutureExt as _;
use thiserror::Error;
use upwell_config::ConfigOverlay;
use upwell_tooling_schema::{
    BinaryTargetIdentity, DocumentIdentity, PackageIdentity, ProbeEnvelope, ProbeFailure,
    ProbeTargetIdentity, TOOLING_PROBE_TRANSITION_PREVIEW_ENV, ToolingDocument,
    TransitionPreviewRequest,
};

#[cfg(feature = "cli")]
use super::failure::bootstrap_failure;
use super::failure::{
    self, details_diagnostic, framework_failure, lifecycle_failure_code, lifecycle_failure_message,
};
use super::{ToolingProbeTargetError, ToolingProjectionError};
use crate::{
    AppHost, BootstrapContext, ExecutionMode, PhaseError, PreparedApp, ProtocolDefinition,
};

/// A typed failure while preparing and projecting a generated application target.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(super) enum ToolingProbeError {
    /// Framework bootstrap could not resolve directories, configuration, or logging policy.
    #[cfg(feature = "cli")]
    #[error("failed to bootstrap the tooling probe: {0}")]
    Bootstrap(#[source] crate::BootstrapError),
    /// The retained parser-visible plugin catalog could not be resolved.
    #[error("failed to resolve the tooling plugin catalog: {0}")]
    PluginCatalog(#[source] crate::Error),
    /// The generated effective CLI parser could not be composed.
    #[cfg(feature = "cli")]
    #[error("failed to compose the tooling CLI definition: {0}")]
    CliDefinition(#[source] crate::CliDefinitionError),
    /// The generated framework parser rejected its own application defaults.
    #[cfg(feature = "cli")]
    #[error("failed to parse generated tooling bootstrap defaults: {0}")]
    CliParse(#[source] clap::Error),
    /// Application setup, configuration, validation, or preparation failed.
    #[error(transparent)]
    Lifecycle(#[from] PhaseError),
    /// The immutable prepared state could not be projected.
    #[error(transparent)]
    Projection(#[from] ToolingProjectionError),
    /// A lifecycle callback or framework future panicked while being polled.
    #[error("the tooling probe panicked")]
    Panic,
    /// The invoker-requested transition preview could not be decoded.
    #[error("the requested transition preview is invalid")]
    PreviewRequest,
}

impl ToolingProbeError {
    pub(super) fn failure(&self) -> ProbeFailure {
        if let Some(failure) = self.structured_failure() {
            return failure;
        }

        let (code, message, phase, resources, sources, hint) = match self {
            #[cfg(feature = "cli")]
            Self::Bootstrap(error) => bootstrap_failure(error),
            Self::PluginCatalog(error) => framework_failure(
                error,
                "upwell/tooling-plugin-catalog",
                "The tooling plugin catalog could not be resolved.",
                None,
            ),
            #[cfg(feature = "cli")]
            Self::CliDefinition(error) => (
                "upwell/tooling-cli-definition",
                "The generated command-line definition is structurally invalid.",
                None,
                vec![format!("cli-command:{}", error.command())],
                Vec::new(),
                Some("Rename or remove the conflicting command-line declaration."),
            ),
            #[cfg(feature = "cli")]
            Self::CliParse(_) => (
                "upwell/tooling-cli-bootstrap",
                "The generated composed parser rejected its tooling bootstrap arguments.",
                None,
                Vec::new(),
                Vec::new(),
                Some("Correct the generated application CLI defaults and retry the probe."),
            ),
            Self::Lifecycle(error) => framework_failure(
                error
                    .source()
                    .expect("phase errors always expose their source"),
                lifecycle_failure_code(error.phase()),
                lifecycle_failure_message(error.phase()),
                Some(error.phase().to_string()),
            ),
            Self::Projection(_) => (
                "upwell/tooling-projection",
                "The prepared application could not be projected into a valid tooling document.",
                None,
                Vec::new(),
                Vec::new(),
                Some("Correct the reported application metadata and retry the probe."),
            ),
            Self::PreviewRequest => (
                "upwell/tooling-preview-request",
                "The requested transition preview is not a valid set of config assignments.",
                None,
                Vec::new(),
                Vec::new(),
                Some("Assign scalar values to dot-separated [A-Za-z0-9_-] property paths."),
            ),
            Self::Panic => (
                "upwell/tooling-panic",
                "The tooling probe panicked while preparing the application.",
                None,
                Vec::new(),
                Vec::new(),
                Some("Run the selected target normally to investigate the application failure."),
            ),
        };

        ProbeFailure {
            phase,
            diagnostics: vec![details_diagnostic(code, message, resources, sources, hint)],
            resource_kinds: BTreeMap::new(),
        }
    }

    fn structured_failure(&self) -> Option<ProbeFailure> {
        match self {
            Self::PluginCatalog(error) => failure::app_diagnostics(error, None),
            #[cfg(feature = "cli")]
            Self::CliDefinition(error) => Some(ProbeFailure {
                phase: None,
                diagnostics: vec![failure::cli_definition_diagnostic(error)],
                resource_kinds: BTreeMap::new(),
            }),
            Self::Lifecycle(error) => error
                .source()
                .and_then(|source| source.downcast_ref::<crate::Error>())
                .and_then(|source| {
                    failure::app_diagnostics(source, Some(error.phase().to_string()))
                }),
            _ => None,
        }
    }
}

/// Runs the target-local preparation path and always returns a serializable probe envelope.
#[doc(hidden)]
pub async fn probe_host<H: AppHost>(identity: DocumentIdentity) -> ProbeEnvelope {
    let result = match requested_preview() {
        Ok(preview) => {
            try_probe_host::<H>(BootstrapContext::new(ExecutionMode::Tooling), preview).await
        }
        Err(error) => Err(error),
    };

    probe_envelope(identity, result)
}

/// Runs a target-local probe after generated bootstrap resolved policy and environment sources.
#[cfg(feature = "cli")]
#[doc(hidden)]
pub async fn probe_bootstrapped_host<H: AppHost>(
    identity: DocumentIdentity,
    context: Result<BootstrapContext, crate::CliError>,
) -> ProbeEnvelope {
    let result = match context {
        Ok(context) => match requested_preview() {
            Ok(preview) => try_probe_bootstrapped_host::<H>(context, preview).await,
            Err(error) => Err(error),
        },
        Err(crate::CliError::Bootstrap(error)) => Err(ToolingProbeError::Bootstrap(error)),
        Err(crate::CliError::Clap(error)) => Err(ToolingProbeError::CliParse(error)),
        Err(crate::CliError::PluginCatalog(error)) => Err(ToolingProbeError::PluginCatalog(error)),
        Err(crate::CliError::Definition(error)) => Err(ToolingProbeError::CliDefinition(error)),
        Err(error) => unreachable!("tooling CLI preparation returned unrelated failure: {error}"),
    };

    probe_envelope(identity, result)
}

fn probe_envelope(
    identity: DocumentIdentity,
    result: Result<ToolingDocument, ToolingProbeError>,
) -> ProbeEnvelope {
    match result {
        Ok(document) => ProbeEnvelope::success(document, identity),
        Err(error) => ProbeEnvelope::failure(identity, error.failure()),
    }
}

/// Reads the Cargo package and binary target explicitly selected by the tooling invoker.
///
/// This contract deliberately does not inspect `CARGO_CRATE_NAME`: a named application may be
/// expanded in a library while the future Cargo invoker selects a thin binary target.
pub fn probe_target_identity_from_env() -> Result<ProbeTargetIdentity, ToolingProbeTargetError> {
    use upwell_tooling_schema::{
        TOOLING_PROBE_BINARY_NAME_ENV, TOOLING_PROBE_MANIFEST_PATH_ENV,
        TOOLING_PROBE_PACKAGE_NAME_ENV, TOOLING_PROBE_PACKAGE_VERSION_ENV,
    };

    let package_name = required_env(TOOLING_PROBE_PACKAGE_NAME_ENV)?;
    let package_version = required_env(TOOLING_PROBE_PACKAGE_VERSION_ENV)?;
    let manifest_path = required_env(TOOLING_PROBE_MANIFEST_PATH_ENV)?;

    if !Path::new(&manifest_path).is_absolute() {
        return Err(ToolingProbeTargetError::RelativeManifestPath);
    }

    let package = PackageIdentity {
        name: package_name,
        version: Some(package_version),
        manifest_path: Some(manifest_path),
    };
    let binary = BinaryTargetIdentity {
        name: required_env(TOOLING_PROBE_BINARY_NAME_ENV)?,
    };

    Ok(ProbeTargetIdentity::new(package, binary)?)
}

/// Polls the complete generated probe under an unwind boundary and sanitizes panic payloads.
///
/// This callable seam does not replace the embedding process's global panic hook. A panic is
/// converted into a stable failure envelope, but the existing hook may already have observed and
/// rendered its payload. The generated dedicated-process runner installs the sanitized process hook
/// before calling this function.
#[doc(hidden)]
pub async fn catch_probe_panic<F>(identity: DocumentIdentity, future: F) -> ProbeEnvelope
where
    F: Future<Output = ProbeEnvelope>,
{
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(envelope) => envelope,
        Err(_) => ProbeEnvelope::failure(identity, ToolingProbeError::Panic.failure()),
    }
}

fn required_env(name: &'static str) -> Result<String, ToolingProbeTargetError> {
    let value =
        std::env::var_os(name).ok_or(ToolingProbeTargetError::MissingEnvironment { name })?;

    let value = value
        .into_string()
        .map_err(|_| ToolingProbeTargetError::InvalidEnvironment { name })?;

    if value.trim().is_empty() {
        return Err(ToolingProbeTargetError::EmptyEnvironment { name });
    }

    Ok(value)
}

async fn try_probe_host<H: AppHost>(
    mut context: BootstrapContext,
    preview: Option<ConfigOverlay>,
) -> Result<ToolingDocument, ToolingProbeError> {
    let plugins =
        crate::resolve_host_plugin_catalog::<H>().map_err(ToolingProbeError::PluginCatalog)?;

    crate::retain_host_plugin_catalog(&mut context, plugins);

    let (_, prepared) = crate::prepare_host_context::<H>(context).await?;

    project_prepared(&prepared, preview.as_ref())
}

#[cfg(feature = "cli")]
async fn try_probe_bootstrapped_host<H: AppHost>(
    context: BootstrapContext,
    preview: Option<ConfigOverlay>,
) -> Result<ToolingDocument, ToolingProbeError> {
    let (_, prepared) = crate::prepare_host_context::<H>(context).await?;

    project_prepared(&prepared, preview.as_ref())
}

/// Projects `prepared`, then previews the requested config change against it.
pub(super) fn project_prepared<D: ProtocolDefinition>(
    prepared: &PreparedApp<D>,
    preview: Option<&ConfigOverlay>,
) -> Result<ToolingDocument, ToolingProbeError> {
    let mut document = prepared.tooling_document()?;

    document.transition_preview = preview.map(|overlay| prepared.transition_preview(overlay));

    Ok(document)
}

/// The config overlay of the transition preview the invoker requested, if any.
fn requested_preview() -> Result<Option<ConfigOverlay>, ToolingProbeError> {
    let Some(value) = std::env::var_os(TOOLING_PROBE_TRANSITION_PREVIEW_ENV) else {
        return Ok(None);
    };

    let request = value
        .to_str()
        .ok_or(ToolingProbeError::PreviewRequest)
        .and_then(|value| {
            TransitionPreviewRequest::from_env_value(value)
                .map_err(|_| ToolingProbeError::PreviewRequest)
        })?;

    preview_overlay(&request).map(Some)
}

pub(super) fn preview_overlay(
    request: &TransitionPreviewRequest,
) -> Result<ConfigOverlay, ToolingProbeError> {
    let mut overlay = ConfigOverlay::default();

    for assignment in &request.assignments {
        overlay
            .set(&assignment.path, &assignment.value)
            .map_err(|_| ToolingProbeError::PreviewRequest)?;
    }

    Ok(overlay)
}
