//! Read-only projection of prepared application plans into the tooling schema.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error as _;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use futures::FutureExt as _;
use thiserror::Error;
use upwell_di::ProviderDescriptor;
use upwell_tooling_schema::{
    BinaryTargetIdentity, Diagnostic, DiagnosticSeverity, DocumentIdentity, PackageIdentity,
    ProbeEnvelope, ProbeFailure, ProbeTargetIdentity, Provenance, RelationshipKind, Resource,
    ResourceKind, ToolingDocument,
};

use crate::{
    AppHost, BootstrapContext, ContributionProvenance, Contributor, ExecutionMode,
    InstallationOrigin, InstallationProvenance, LifecyclePhase, PhaseError, PreparedApp,
    ProtocolDefinition,
};

mod composition;
mod contribution;
mod failure;
mod output;
mod panic;
mod relationship;
mod resources;
mod snapshot;
mod source;

pub(crate) use contribution::ToolingContributionSet;
pub use contribution::{
    ToolingContributionError, ToolingContributions, ToolingEndpoint, ToolingRelationshipKind,
};
pub use output::{
    ToolingProbeOutputError, ToolingProbeOutputTargetError, emit_probe_envelope,
    emit_probe_envelope_from_env,
};
pub use panic::install_process_probe_panic_hook;
pub(crate) use snapshot::ProjectionSnapshot;
pub(crate) use source::{item_location, type_location, with_source};
pub use upwell_tooling_schema::ResourceDisplay;

/// A typed failure while projecting an already prepared application.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolingProjectionError {
    /// A prepared protocol or plugin supplied invalid owner-scoped metadata.
    #[error(transparent)]
    Contribution(#[from] ToolingContributionError),
}

/// A typed failure while preparing and projecting a generated application target.
#[derive(Debug, Error)]
#[non_exhaustive]
enum ToolingProbeError {
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
}

/// A typed failure while resolving the selected target for a generated tooling entry.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolingProbeTargetError {
    /// A required invoker-owned environment variable is absent.
    #[error("required tooling probe environment variable '{name}' is not set")]
    MissingEnvironment {
        /// Missing variable name.
        name: &'static str,
    },
    /// An invoker-owned environment variable is not valid Unicode.
    #[error("tooling probe environment variable '{name}' is not valid Unicode")]
    InvalidEnvironment {
        /// Invalid variable name.
        name: &'static str,
    },
    /// An invoker-owned environment variable is empty or contains only whitespace.
    #[error("tooling probe environment variable '{name}' is empty")]
    EmptyEnvironment {
        /// Empty variable name.
        name: &'static str,
    },
    /// The Cargo manifest path is not an absolute path.
    #[error("tooling probe manifest path must be absolute")]
    RelativeManifestPath,
    /// The invoker-owned package or binary target identity is incomplete.
    #[error(transparent)]
    InvalidIdentity(#[from] upwell_tooling_schema::IdentityValidationError),
}

impl ToolingProbeError {
    fn failure(&self) -> ProbeFailure {
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
            Self::Panic => (
                "upwell/tooling-panic",
                "The tooling probe panicked while preparing the application.",
                None,
                Vec::new(),
                Vec::new(),
                Some("Run the selected target normally to investigate the application failure."),
            ),
        };

        let diagnostic = Diagnostic {
            code: code.to_string(),
            severity: DiagnosticSeverity::Error,
            message: message.to_string(),
            resources,
            sources,
            fix: hint.map(str::to_string),
        };

        ProbeFailure {
            phase,
            diagnostics: vec![diagnostic],
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
    probe_host_context::<H>(identity, BootstrapContext::new(ExecutionMode::Tooling)).await
}

/// Runs a target-local probe after generated bootstrap resolved policy and environment sources.
#[cfg(feature = "cli")]
#[doc(hidden)]
pub async fn probe_bootstrapped_host<H: AppHost>(
    identity: DocumentIdentity,
    context: Result<BootstrapContext, crate::CliError>,
) -> ProbeEnvelope {
    let result = match context {
        Ok(context) => try_probe_bootstrapped_host::<H>(context).await,
        Err(crate::CliError::Bootstrap(error)) => Err(ToolingProbeError::Bootstrap(error)),
        Err(crate::CliError::Clap(error)) => Err(ToolingProbeError::CliParse(error)),
        Err(crate::CliError::PluginCatalog(error)) => Err(ToolingProbeError::PluginCatalog(error)),
        Err(crate::CliError::Definition(error)) => Err(ToolingProbeError::CliDefinition(error)),
        Err(error) => unreachable!("tooling CLI preparation returned unrelated failure: {error}"),
    };

    probe_envelope(identity, result)
}

async fn probe_host_context<H: AppHost>(
    identity: DocumentIdentity,
    context: BootstrapContext,
) -> ProbeEnvelope {
    let result = try_probe_host::<H>(context).await;

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

type FailureDetails = (
    &'static str,
    &'static str,
    Option<String>,
    Vec<String>,
    Vec<upwell_tooling_schema::SourceLocation>,
    Option<&'static str>,
);

#[cfg(feature = "cli")]
fn bootstrap_failure(error: &crate::BootstrapError) -> FailureDetails {
    match error {
        crate::BootstrapError::Directories(_) => (
            "upwell/tooling-directories",
            "Application directories could not be resolved for the tooling probe.",
            None,
            Vec::new(),
            Vec::new(),
            Some("Verify the process environment permits resolving application directories."),
        ),
        crate::BootstrapError::Config(error) => config_failure(error, None),
        crate::BootstrapError::LogFormat { .. } => (
            "upwell/tooling-log-format",
            "The configured tooling log format is not supported.",
            None,
            Vec::new(),
            Vec::new(),
            Some("Use a formatter listed by the generated --log-format help."),
        ),
        crate::BootstrapError::MissingConfigPath { .. } => (
            "upwell/tooling-config-path-missing",
            "The selected configuration path does not exist.",
            None,
            Vec::new(),
            Vec::new(),
            Some("Supply an existing configuration file or directory."),
        ),
        #[cfg(feature = "tracing-subscriber")]
        crate::BootstrapError::Tracing(_) => (
            "upwell/tooling-tracing",
            "Tracing could not be initialized for the tooling probe.",
            None,
            Vec::new(),
            Vec::new(),
            Some("Remove the conflicting global tracing subscriber or disable probe logging."),
        ),
    }
}

fn framework_failure(
    error: &(dyn std::error::Error + 'static),
    fallback_code: &'static str,
    fallback_message: &'static str,
    phase: Option<String>,
) -> FailureDetails {
    if let Some(error) = error.downcast_ref::<crate::Error>() {
        return app_failure(error, phase);
    }

    (
        fallback_code,
        fallback_message,
        phase,
        Vec::new(),
        Vec::new(),
        Some("Run the selected target normally to investigate the application failure."),
    )
}

fn app_failure(error: &crate::Error, phase: Option<String>) -> FailureDetails {
    match error {
        crate::Error::MissingConfig {
            component, path, ..
        } => (
            "upwell/tooling-config-binding-missing",
            "A component requires a configuration binding that is not registered.",
            phase,
            vec![component_resource(component), config_resource(path)],
            Vec::new(),
            Some("Register the missing configuration binding or name an existing binding."),
        ),
        crate::Error::AmbiguousConfig { component, .. } => (
            "upwell/tooling-config-binding-ambiguous",
            "A component configuration binding is ambiguous.",
            phase,
            vec![component_resource(component)],
            Vec::new(),
            Some("Name the intended configuration path explicitly."),
        ),
        crate::Error::Config(error) => config_failure(error, phase),
        crate::Error::Di(error) => failure::di_failure(error, phase),
        crate::Error::Hook(_) => (
            "upwell/tooling-hook",
            "A framework lifecycle hook could not be prepared.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Run the selected target normally to investigate the hook failure."),
        ),
        crate::Error::Composition(_) => (
            "upwell/tooling-plugin-composition",
            "Plugin declarations could not be composed into a deterministic plan.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Correct the conflicting plugin declarations or dependencies."),
        ),
        crate::Error::PluginPlan(_) => (
            "upwell/tooling-plugin-plan",
            "Plugin contributions could not be lowered into the application plan.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Correct duplicate or reserved plugin contribution identities."),
        ),
        crate::Error::ScopeTopology(error) => (
            "upwell/tooling-scope-topology",
            "The protocol scope topology is structurally invalid.",
            phase,
            failure::scope_topology_resources(error),
            Vec::new(),
            Some("Correct duplicate, missing, cyclic, or invalid scope parent declarations."),
        ),
        crate::Error::UndeclaredScope {
            component, scope, ..
        } => (
            "upwell/tooling-scope-undeclared",
            "A component refers to a scope absent from the selected protocol.",
            phase,
            vec![component_resource(component), format!("scope:{scope}")],
            Vec::new(),
            Some("Declare the scope in the protocol topology or change the component scope."),
        ),
        _ => (
            "upwell/tooling-framework",
            "The framework could not prepare the application tooling plan.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Run the selected target normally to investigate the framework failure."),
        ),
    }
}

fn config_failure(error: &upwell_config::ConfigError, phase: Option<String>) -> FailureDetails {
    match error {
        upwell_config::ConfigError::Io { .. } => (
            "upwell/tooling-config-read",
            "A configuration source could not be read.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Verify that the configuration source exists and is readable."),
        ),
        upwell_config::ConfigError::Parse { .. } => (
            "upwell/tooling-config-parse",
            "A configuration source could not be parsed.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Correct the configuration syntax or unresolved placeholders."),
        ),
        upwell_config::ConfigError::UnsupportedFormat { .. } => (
            "upwell/tooling-config-format",
            "A configuration source uses an unsupported format.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Use a configuration format enabled for the selected target."),
        ),
        upwell_config::ConfigError::MissingPath { path } => (
            "upwell/tooling-config-value-missing",
            "A required configuration value is absent.",
            phase,
            vec![config_resource(path)],
            Vec::new(),
            Some("Define the required configuration path in an active source."),
        ),
        upwell_config::ConfigError::Substitution { path, .. } => (
            "upwell/tooling-config-substitution",
            "A configuration placeholder could not be resolved.",
            phase,
            vec![config_resource(path)],
            Vec::new(),
            Some("Define the referenced value without exposing it through probe output."),
        ),
        _ => (
            "upwell/tooling-config",
            "Configuration could not be prepared for tooling.",
            phase,
            Vec::new(),
            Vec::new(),
            Some("Run the selected target normally to investigate the configuration failure."),
        ),
    }
}

fn lifecycle_failure_message(phase: LifecyclePhase) -> &'static str {
    match phase {
        LifecyclePhase::Setup => "Application setup failed during the tooling probe.",
        LifecyclePhase::Configure => "Application configuration failed during the tooling probe.",
        LifecyclePhase::BeforeBuild => {
            "Application pre-build preparation failed during the tooling probe."
        }
        LifecyclePhase::Prepare => "Application planning failed during the tooling probe.",
        LifecyclePhase::Build => "Application construction failed during the tooling probe.",
        LifecyclePhase::AfterBuild => {
            "Application post-build processing failed during the tooling probe."
        }
        LifecyclePhase::Serve => "Application serving failed during the tooling probe.",
    }
}

fn component_resource(component: &str) -> String {
    format!("component:{component}")
}

fn config_resource(path: &str) -> String {
    format!("config:{path}")
}

async fn try_probe_host<H: AppHost>(
    context: BootstrapContext,
) -> Result<ToolingDocument, ToolingProbeError> {
    let plugins =
        crate::resolve_host_plugin_catalog::<H>().map_err(ToolingProbeError::PluginCatalog)?;

    try_probe_host_with_plugins::<H>(context, plugins).await
}

async fn try_probe_host_with_plugins<H: AppHost>(
    mut context: BootstrapContext,
    plugins: crate::EarlyPluginCatalog,
) -> Result<ToolingDocument, ToolingProbeError> {
    crate::retain_host_plugin_catalog(&mut context, plugins);

    let (_, prepared) = crate::prepare_host_context::<H>(context).await?;

    Ok(prepared.tooling_document()?)
}

#[cfg(feature = "cli")]
async fn try_probe_bootstrapped_host<H: AppHost>(
    context: BootstrapContext,
) -> Result<ToolingDocument, ToolingProbeError> {
    let (_, prepared) = crate::prepare_host_context::<H>(context).await?;

    Ok(prepared.tooling_document()?)
}

fn lifecycle_failure_code(phase: LifecyclePhase) -> &'static str {
    match phase {
        LifecyclePhase::Setup => "upwell/tooling-setup",
        LifecyclePhase::Configure => "upwell/tooling-configure",
        LifecyclePhase::BeforeBuild => "upwell/tooling-before-build",
        LifecyclePhase::Prepare => "upwell/tooling-prepare",
        LifecyclePhase::Build => "upwell/tooling-build",
        LifecyclePhase::AfterBuild => "upwell/tooling-after-build",
        LifecyclePhase::Serve => "upwell/tooling-serve",
    }
}

impl<D: ProtocolDefinition> PreparedApp<D> {
    /// Projects the exact immutable prepared plan without constructing runtime state.
    pub fn tooling_document(&self) -> Result<ToolingDocument, ToolingProjectionError> {
        let mut projection = Projection::new(self);

        projection.project()?;

        Ok(projection.document)
    }
}

struct Projection<'a, D: ProtocolDefinition> {
    app: &'a PreparedApp<D>,
    document: ToolingDocument,
    type_resources: BTreeSet<String>,
    relationship_indexes: HashMap<(RelationshipKind, String, String), usize>,
}

impl<'a, D: ProtocolDefinition> Projection<'a, D> {
    fn new(app: &'a PreparedApp<D>) -> Self {
        let identity = DocumentIdentity {
            application: app.name().to_string(),
            ..DocumentIdentity::default()
        };

        Self {
            app,
            document: ToolingDocument::new(
                env!("CARGO_PKG_VERSION"),
                identity,
                app.protocol_id().as_str(),
            ),
            type_resources: BTreeSet::new(),
            relationship_indexes: HashMap::new(),
        }
    }

    fn project(&mut self) -> Result<(), ToolingProjectionError> {
        self.project_application();
        self.project_scopes();
        self.project_components();
        self.project_providers();
        self.project_config_bindings();
        self.project_hooks();
        self.project_plugins();
        self.project_cli();
        self.project_tooling_contributions()?;

        Ok(())
    }

    fn inactive_plugin_resource(
        &mut self,
        id: &str,
        name: &str,
        provenance: InstallationProvenance,
        decision: &str,
    ) {
        if self
            .document
            .resources
            .iter()
            .any(|resource| resource.id == id)
        {
            return;
        }

        self.resource(
            id,
            ResourceKind::Plugin,
            name,
            Some(installation_provenance(provenance)),
            BTreeMap::from([
                (String::from("effective"), String::from("false")),
                (String::from("decision"), decision.to_string()),
            ]),
        );
    }

    fn resource(
        &mut self,
        id: &str,
        kind: ResourceKind,
        name: &str,
        provenance: Option<Provenance>,
        labels: BTreeMap<String, String>,
    ) {
        self.document.resources.push(Resource {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            display: None,
            provenance,
            labels,
            facets: BTreeMap::new(),
        });
    }

    /// The type's graph endpoint, declared on first use. A later descriptor that recorded the
    /// type's declaration backfills the source of an endpoint first seen only by reference.
    fn type_resource(&mut self, ty: &upwell_core::TypeDescriptor) -> String {
        let rust_type = (ty.type_name)();
        let id = type_id(rust_type);
        let source = type_location(ty);

        if self.type_resources.insert(id.clone()) {
            self.resource(
                &id,
                ResourceKind::Type,
                ty.name,
                with_source(None, source),
                BTreeMap::from([(String::from("rust-type"), rust_type.to_string())]),
            );
        } else if source.is_some() {
            let resource = self
                .document
                .resources
                .iter_mut()
                .find(|resource| resource.id == id)
                .expect("declared type resource exists");

            if resource.provenance.is_none() {
                resource.provenance = with_source(None, source);
            }
        }

        id
    }
}

fn installation_provenance(provenance: InstallationProvenance) -> Provenance {
    let (owner, origin) = match provenance.origin() {
        InstallationOrigin::ProtocolMandatory(protocol) => (
            Some(format!("protocol:{}", protocol.as_str())),
            "protocol-mandatory",
        ),
        InstallationOrigin::ProtocolDefault(protocol) => (
            Some(format!("protocol:{}", protocol.as_str())),
            "protocol-default",
        ),
        InstallationOrigin::ApplicationDeclaration => {
            (Some(String::from("application")), "application-declaration")
        }
        InstallationOrigin::ApplicationConfiguration => (
            Some(String::from("application")),
            "application-configuration",
        ),
    };

    Provenance {
        owner,
        origin: Some(origin.to_string()),
        ordinal: Some(provenance.ordinal()),
        ..Provenance::default()
    }
}

fn contribution_provenance(provenance: ContributionProvenance) -> Provenance {
    Provenance {
        owner: Some(contributor_id(provenance.contributor())),
        origin: Some(String::from("contribution")),
        ..Provenance::default()
    }
}

fn contributor_id(contributor: Contributor) -> String {
    match contributor {
        Contributor::Framework => String::from("framework"),
        Contributor::Application => String::from("application"),
        Contributor::Protocol(protocol) => format!("protocol:{}", protocol.as_str()),
        Contributor::Plugin(plugin) => plugin_id(plugin.as_str()),
    }
}

fn contribution_id(provenance: ContributionProvenance) -> String {
    upwell_tooling_schema::contribution_id(
        &contributor_id(provenance.contributor()),
        provenance.contribution().as_str(),
    )
}

fn lifecycle_resource(kind: &str) -> Option<&'static str> {
    match kind {
        "startup" => Some("lifecycle:startup"),
        "shutdown" => Some("lifecycle:shutdown"),
        "config_reload" => Some("lifecycle:config_reload"),
        _ => None,
    }
}

fn component_id(id: &str) -> String {
    format!("component:{id}")
}

fn provider_id(provider: &ProviderDescriptor) -> String {
    format!(
        "provider:{}:{}:{}",
        (provider.trait_ty.type_name)(),
        (provider.concrete_ty.type_name)(),
        provider.qualifier
    )
}

fn config_binding_id(rust_type: &str, path: &str) -> String {
    format!("config-binding:{rust_type}:{path}")
}

fn plugin_id(id: &str) -> String {
    format!("plugin:{id}")
}

fn scope_id(id: &str) -> String {
    format!("scope:{id}")
}

fn type_id(rust_type: &str) -> String {
    format!("type:{rust_type}")
}

#[cfg(test)]
mod tests;
