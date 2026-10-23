use upwell_tooling_schema::{Diagnostic, DiagnosticSeverity};

use crate::LifecyclePhase;
use crate::tooling::{component_resource, config_resource};

pub(in crate::tooling) fn details_diagnostic(
    code: &str,
    message: &str,
    resources: Vec<String>,
    sources: Vec<upwell_tooling_schema::SourceLocation>,
    hint: Option<&str>,
) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        severity: DiagnosticSeverity::Error,
        message: message.to_string(),
        resources,
        sources,
        fix: hint.map(str::to_string),
    }
}

pub(in crate::tooling) type FailureDetails = (
    &'static str,
    &'static str,
    Option<String>,
    Vec<String>,
    Vec<upwell_tooling_schema::SourceLocation>,
    Option<&'static str>,
);

#[cfg(feature = "cli")]
pub(in crate::tooling) fn bootstrap_failure(error: &crate::BootstrapError) -> FailureDetails {
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

pub(in crate::tooling) fn framework_failure(
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

pub(in crate::tooling) fn app_failure(
    error: &crate::Error,
    phase: Option<String>,
) -> FailureDetails {
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
        crate::Error::Di(error) => super::di_failure(error, phase),
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
            super::scope_topology_resources(error),
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

pub(in crate::tooling) fn lifecycle_failure_message(phase: LifecyclePhase) -> &'static str {
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

pub(in crate::tooling) fn lifecycle_failure_code(phase: LifecyclePhase) -> &'static str {
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
