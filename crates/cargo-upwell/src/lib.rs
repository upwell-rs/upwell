//! Cargo target discovery and probe execution for Upwell developer tooling.
//!
//! This crate contains presentation-neutral orchestration and command reports shared by the
//! `cargo upwell` subcommand and editor integrations. Terminal rendering remains in the binary.

mod build;
mod command;
pub mod completion;
mod discovery;
pub mod graph;
mod init;
mod probe;
mod process;
mod renderer;
mod selection;

use std::fs::OpenOptions;

pub use build::{BuildError, BuildEvidence, BuildResult, CargoDiagnostic};
use build::{BuildRequest, build_target};
pub use command::{
    CommandCheck, CommandCheckStatus, CommandExitCode, CommandKind, CommandOutcome, CommandReport,
    SelectedTargetReport, probe_request_exit_code, run_command, run_command_with_options,
};
pub use discovery::{CargoExecutable, DiscoveryError, DiscoveryRequest, discover};
use fs2::FileExt as _;
pub use graph::{
    CliArgumentOwnership, CliCommandOwnership, CliOwnershipSummary, ConditionStep, GraphDirection,
    GraphEmitError, GraphQuery, GraphQueryError, GraphRelationFamily, GraphSelectorKind,
    GraphSource, GraphView, ProviderSelection, ResolvedConsumer, ResourceExplanation,
    explain_resource, query_failure_graph, query_graph,
};
pub use init::{
    Catalog, CatalogError, GitReference, InitError, InitRequest, InitResult, TemplateEntry,
    TemplateSelection, TemplateSource, ToolEntry, default_catalog_path, init_project,
};
use probe::execute_probe;
pub use probe::{ProbeError, ProbeEvidence, ProbeResult};
pub use process::{CancellationToken, ProcessStatus};
pub use renderer::{
    BuiltInRenderer, ComponentLimits, ComponentRenderError, ComponentRenderRequest,
    ComponentRenderer, ComponentRendererHost, RENDERER_ABI_REQUIREMENT, RendererCapabilities,
    RendererCommand, RendererDescriptor, RendererFormat, RendererImplementation, RendererRegistry,
    RendererRegistryError, ResolvedRenderer, run_component_compiler_worker,
};
pub use selection::{
    BinaryCandidate, FeatureSelection, PackageCandidate, SelectedTarget, SelectionError,
    WorkspaceCatalog,
};
pub use upwell_tooling_schema::TOOLING_SCHEMA_VERSION;

use thiserror::Error;

/// Complete selected-target probe result returned without presentation policy.
#[derive(Debug)]
pub struct ToolingProbe {
    /// Cargo workspace facts used for selection and artifact isolation.
    pub workspace: WorkspaceCatalog,
    /// Selected package and binary identity.
    pub target: SelectedTarget,
    /// Cargo build result and diagnostics.
    pub build: BuildResult,
    /// Validated target-local probe response and process evidence.
    pub probe: ProbeResult,
}

/// Failure from the complete discover, build, and probe orchestration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeRequestError {
    /// Cargo workspace discovery or target selection failed.
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    /// The selected target could not be built.
    #[error("{source}")]
    Build {
        /// Target selected before the build failed.
        target: Box<SelectedTarget>,
        /// Typed Cargo build failure.
        #[source]
        source: Box<BuildError>,
    },
    /// The selected executable did not complete the private probe contract.
    #[error("the selected executable did not complete the private tooling probe")]
    Probe {
        /// Target selected before probe execution failed.
        target: Box<SelectedTarget>,
        /// Typed target-local probe failure.
        #[source]
        source: Box<ProbeError>,
    },
    /// Tooling invocation serialization could not be established.
    #[error("failed to acquire the Cargo tooling invocation lock")]
    Lock(#[source] std::io::Error),
    /// Cancellation was requested while waiting for another tooling invocation.
    #[error("tooling probe was cancelled while waiting for the invocation lock")]
    LockCancelled,
}

/// Discovers, builds, and executes one selected Upwell application probe.
pub fn run_probe(
    request: &DiscoveryRequest,
    cancellation: &CancellationToken,
) -> Result<ToolingProbe, ProbeRequestError> {
    run_probe_with_options(request, cancellation, ProbeOptions::default())
}

/// Presentation and request options for one complete application probe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProbeOptions {
    /// Show Cargo's human-readable build output on this process's stderr.
    pub show_cargo_output: bool,
    /// A proposed config change the probe previews against the prepared application.
    pub transition_preview: Option<upwell_tooling_schema::TransitionPreviewRequest>,
}

/// Discovers, builds, and executes one selected application probe with presentation options.
pub fn run_probe_with_options(
    request: &DiscoveryRequest,
    cancellation: &CancellationToken,
    options: ProbeOptions,
) -> Result<ToolingProbe, ProbeRequestError> {
    let mut discovered = discover(request, cancellation)?;
    let (workspace, target, _lock) = loop {
        let lock = InvocationLock::acquire(&discovered.0.target_directory, cancellation)?;

        if !lock.was_contended() {
            break (discovered.0, discovered.1, lock);
        }

        let refreshed = discover(request, cancellation)?;

        if refreshed.0.target_directory == discovered.0.target_directory {
            break (refreshed.0, refreshed.1, lock);
        }

        drop(lock);
        discovered = refreshed;
    };

    let build = build_target(
        &BuildRequest {
            cargo: request.cargo.clone(),
            current_dir: request.current_dir.clone(),
            target: request.target.clone(),
            features: request.features.clone(),
            workspace_target_directory: workspace.target_directory.clone(),
            selected: target.clone(),
            show_cargo_output: options.show_cargo_output,
        },
        cancellation,
    )
    .map_err(|source| ProbeRequestError::Build {
        target: Box::new(target.clone()),
        source: Box::new(source),
    })?;
    let mut probe = execute_probe(
        &build.executable,
        &workspace.target_directory,
        &target,
        Some(&workspace.workspace_root),
        options.transition_preview.as_ref(),
        cancellation,
    )
    .map_err(|source| ProbeRequestError::Probe {
        target: Box::new(target.clone()),
        source: Box::new(source),
    })?;

    probe
        .envelope
        .set_workspace_root(workspace.workspace_root.display().to_string());

    let probe = ToolingProbe {
        workspace,
        target,
        build,
        probe,
    };

    Ok(probe)
}

struct InvocationLock {
    file: std::fs::File,
    contended: bool,
}

impl InvocationLock {
    fn acquire(
        workspace_target_directory: &std::path::Path,
        cancellation: &CancellationToken,
    ) -> Result<Self, ProbeRequestError> {
        Self::acquire_with_wait(workspace_target_directory, cancellation, || {
            std::thread::sleep(std::time::Duration::from_millis(20));
        })
    }

    fn acquire_with_wait(
        workspace_target_directory: &std::path::Path,
        cancellation: &CancellationToken,
        mut wait: impl FnMut(),
    ) -> Result<Self, ProbeRequestError> {
        let directory = workspace_target_directory.join("upwell");
        let path = directory.join("invocation.lock");

        std::fs::create_dir_all(&directory).map_err(ProbeRequestError::Lock)?;

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(ProbeRequestError::Lock)?;

        let mut contended = false;

        loop {
            if cancellation.is_cancelled() {
                return Err(ProbeRequestError::LockCancelled);
            }

            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file, contended }),
                Err(error) if lock_is_contended(&error) => {
                    contended = true;
                    wait();
                }
                Err(error) => return Err(ProbeRequestError::Lock(error)),
            }
        }
    }

    fn was_contended(&self) -> bool {
        self.contended
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    let contended = fs2::lock_contended_error();

    error.raw_os_error() == contended.raw_os_error()
}

impl Drop for InvocationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
