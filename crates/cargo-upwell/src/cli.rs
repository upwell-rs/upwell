use std::ffi::OsString;
use std::path::PathBuf;

use cargo_upwell::{
    CargoExecutable, Catalog, CommandKind, DiscoveryRequest, FeatureSelection, GraphQuery,
    InitRequest, TemplateSelection, completion,
};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{
    Args, ColorChoice, CommandFactory as _, FromArgMatches as _, Parser, Subcommand, ValueHint,
};
use clap_complete::{ArgValueCompleter, CompletionCandidate};
use upwell_tooling_schema::{ConfigAssignment, TransitionPreviewRequest};

mod format;
mod preview;

use format::{
    Check, Doctor, Explain, Export, Format, Graph, GraphFamily, GraphTraversalDirection, Inspect,
};
pub(crate) use format::{
    ExplainFormat, GraphFormat, InspectCliProviderKind, InspectResourceKind, ReportFormat,
    TerminalPolicy,
};

const CARGO_HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::BrightCyan.on_default());

/// Parsed `cargo upwell` process arguments.
#[derive(Debug, Parser)]
#[command(
    name = "cargo upwell",
    bin_name = "cargo upwell",
    version = crate::version::version(),
    about,
    styles = CARGO_HELP_STYLES,
    color = ColorChoice::Auto
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Available Cargo Upwell commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Generates shell integration or refreshes workspace-aware candidates.
    ///
    /// Registration targets the installed `cargo-upwell` executable directly. Source the emitted
    /// script from the shell startup file. Dynamic value completion reads only a private cache;
    /// pressing Tab never invokes Cargo or application code. Successful `inspect`, `export`,
    /// `graph`, and `explain` probes refresh the latest selected target on a best-effort basis;
    /// `refresh` updates it explicitly and reports cache failures.
    /// On macOS snapshots live under
    /// `~/Library/Caches/org.upwell-rs.Upwell/completions/v1/`. They intentionally do not use
    /// Cargo `OUT_DIR`, whose hashed path changes across packages, features, targets, and profiles.
    /// `UPWELL_COMPLETION_CACHE_DIR` may override the platform cache root with an absolute path.
    /// Fish users should install the script under `~/.config/fish/conf.d/` so both direct and
    /// `cargo upwell` completion definitions load at shell startup. Regenerate registration after
    /// upgrading `cargo-upwell`.
    Completions(CompletionsArgs),
    /// Generates an Upwell application, plugin, or protocol project.
    Init(InitArgs),
    /// Lists built-in and user-configured project templates.
    ///
    /// Overlays the optional user catalog onto built-ins by stable ID. The default catalog is
    /// optional; an explicitly supplied `--catalog` path must exist and validate. On macOS the
    /// default is `~/Library/Application Support/org.upwell-rs.Upwell/catalog.toml`. Relative
    /// template paths are resolved from the catalog file. Tool entries are metadata-only and are
    /// never installed or executed.
    Templates(TemplatesArgs),
    /// Builds and validates one selected application without serving it.
    Check(CheckReportArgs),
    /// Diagnoses Cargo selection, build, probe, and application preparation.
    Doctor(DoctorReportArgs),
    /// Displays the selected application's prepared tooling document.
    Inspect(InspectArgs),
    /// Emits the canonical tooling document or probe envelope.
    Export(ExportArgs),
    /// Renders application resources and their relationships.
    Graph(GraphArgs),
    /// Explains one exact resource identity or unique name.
    Explain(ExplainArgs),
}

/// Shell completion management.
#[derive(Clone, Debug, Args)]
struct CompletionsArgs {
    #[command(subcommand)]
    command: CompletionsCommand,
}

/// Shell completion operations.
#[derive(Clone, Debug, Subcommand)]
enum CompletionsCommand {
    /// Emits shell registration code that calls cargo-upwell for dynamic candidates.
    Generate {
        /// Shell whose startup file will source the emitted code.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Builds and probes the selected app, then caches workspace-aware candidates.
    Refresh {
        #[command(flatten)]
        target: TargetArgs,
    },
}

/// Arguments for effective catalog template listing.
#[derive(Clone, Debug, Args)]
struct TemplatesArgs {
    /// Explicit Upwell catalog file.
    #[arg(long, value_hint = ValueHint::FilePath)]
    catalog: Option<PathBuf>,
}

/// Arguments for catalog-backed cargo-generate scaffolding.
#[derive(Clone, Debug, Args)]
struct InitArgs {
    /// Exact project directory to populate.
    #[arg(value_hint = ValueHint::DirPath)]
    path: PathBuf,
    /// Cargo package/project name. Defaults to the destination directory name.
    #[arg(long)]
    name: Option<String>,
    /// Catalog template ID.
    #[arg(long, conflicts_with = "template_path", add = ArgValueCompleter::new(complete_templates))]
    template: Option<String>,
    /// Direct local cargo-generate template directory.
    #[arg(long, conflicts_with_all = ["template", "catalog"], value_hint = ValueHint::DirPath)]
    template_path: Option<PathBuf>,
    /// Explicit Upwell catalog file.
    #[arg(long, value_hint = ValueHint::FilePath)]
    catalog: Option<PathBuf>,
    /// Add the generated crate to an immediate parent Cargo workspace.
    #[arg(long)]
    workspace: bool,
    /// Skip creation of a Git repository.
    #[arg(long)]
    no_vcs: bool,
    /// Supply a cargo-generate template value as key=value.
    #[arg(short = 'd', long = "define", action = clap::ArgAction::Append)]
    define: Vec<String>,
    /// Use a local Upwell repository dependency in built-in templates.
    #[arg(long, value_hint = ValueHint::DirPath)]
    upwell_path: Option<PathBuf>,
}

/// Cargo target selection shared by application commands.
#[derive(Clone, Debug, Args)]
struct TargetArgs {
    /// Path to Cargo.toml.
    #[arg(long, value_hint = ValueHint::FilePath)]
    manifest_path: Option<PathBuf>,
    /// Workspace package containing the application.
    #[arg(short = 'p', long, add = ArgValueCompleter::new(complete_packages))]
    package: Option<String>,
    /// Binary target containing the application.
    #[arg(long = "bin", add = ArgValueCompleter::new(complete_binaries))]
    binary: Option<String>,
    /// Package features enabled for discovery and build.
    #[arg(long, value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_features))]
    features: Vec<String>,
    /// Enable every package feature.
    #[arg(long, conflicts_with = "features")]
    all_features: bool,
    /// Disable package default features.
    #[arg(long)]
    no_default_features: bool,
    /// Cargo build target triple.
    #[arg(long)]
    target: Option<String>,
}

/// Arguments for check and doctor reports.
#[derive(Clone, Debug, Args)]
struct CheckReportArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Check>,
}

/// Arguments for doctor reports.
#[derive(Clone, Debug, Args)]
struct DoctorReportArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Doctor>,
}

/// Arguments for generic application inspection.
#[derive(Clone, Debug, Args)]
struct InspectArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Inspect>,
    #[command(flatten)]
    filters: InspectFilters,
    #[command(flatten)]
    terminal: TerminalArgs,
}

/// Arguments for canonical machine-readable export.
#[derive(Clone, Debug, Args)]
struct ExportArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Export>,
    /// Write to a file instead of stdout.
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    output: Option<PathBuf>,
}

/// Arguments for deterministic graph rendering.
#[derive(Clone, Debug, Args)]
struct GraphArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Graph>,
    /// Semantic relationship family to include.
    #[arg(long, value_enum, default_value_t = GraphFamily::All)]
    family: GraphFamily,
    /// Semantic traversal direction from selected roots.
    #[arg(long, value_enum, default_value_t = GraphTraversalDirection::Both)]
    direction: GraphTraversalDirection,
    /// Select an exact stable resource ID or exact unique name.
    #[arg(long = "resource", action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_resources))]
    resources: Vec<String>,
    /// Select an exact contributor stable ID or exact unique name.
    #[arg(long = "contributor", action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_contributors))]
    contributors: Vec<String>,
    /// Select an exact plugin stable ID or exact unique name.
    #[arg(long = "plugin", action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_plugins))]
    plugins: Vec<String>,
    #[command(flatten)]
    terminal: TerminalArgs,
}

/// Arguments for one resource explanation.
#[derive(Clone, Debug, Args)]
struct ExplainArgs {
    /// Exact stable resource ID or exact unique name.
    #[arg(add = ArgValueCompleter::new(complete_resources))]
    resource: String,
    /// Previews the transition a config change would cause, without constructing anything.
    /// Repeatable; later assignments win. VALUE is `true`/`false`, a number, a `"quoted
    /// string"`, or bare text.
    #[arg(long = "set", value_name = "PATH=VALUE", value_parser = preview::parse_assignment)]
    set: Vec<ConfigAssignment>,
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    format: Format<Explain>,
    #[command(flatten)]
    terminal: TerminalArgs,
}

/// Generic inspection filters combined across dimensions.
#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectFilters {
    /// Include these generic resource kinds.
    #[arg(long = "kind", value_enum, value_delimiter = ',', action = clap::ArgAction::Append)]
    pub(crate) kinds: Vec<InspectResourceKind>,
    /// Include resources with these exact stable IDs or names.
    #[arg(long = "resource", value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_resources))]
    pub(crate) resources: Vec<String>,
    /// Include resources owned by these contributor IDs or names.
    #[arg(long = "contributor", value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_contributors))]
    pub(crate) contributors: Vec<String>,
    /// Include these plugin IDs or names and their owned resources.
    #[arg(long = "plugin", value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_plugins))]
    pub(crate) plugins: Vec<String>,
    /// Include resources carrying these facet namespaces.
    #[arg(long = "facet", value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_facets))]
    pub(crate) facets: Vec<String>,
    /// Include these scope IDs or names and resources assigned to them.
    #[arg(long = "scope", value_delimiter = ',', action = clap::ArgAction::Append, add = ArgValueCompleter::new(complete_scopes))]
    pub(crate) scopes: Vec<String>,
    /// Include these plugin CLI provider categories.
    #[arg(long = "cli-provider-kind", value_enum, value_delimiter = ',', action = clap::ArgAction::Append)]
    pub(crate) cli_provider_kinds: Vec<InspectCliProviderKind>,
}

impl InspectFilters {
    pub(crate) fn is_empty(&self) -> bool {
        self.kinds.is_empty()
            && self.resources.is_empty()
            && self.contributors.is_empty()
            && self.plugins.is_empty()
            && self.facets.is_empty()
            && self.scopes.is_empty()
            && self.cli_provider_kinds.is_empty()
    }
}

/// Terminal presentation policy for human-readable inspection.
#[derive(Clone, Copy, Debug, Args)]
struct TerminalArgs {
    /// ANSI color policy.
    #[arg(long, value_enum, default_value_t = TerminalPolicy::Auto)]
    color: TerminalPolicy,
    /// Pager policy.
    #[arg(long, value_enum, default_value_t = TerminalPolicy::Auto)]
    pager: TerminalPolicy,
}

/// Parsed Cargo Upwell command request.
#[derive(Debug)]
pub(crate) enum CommandRequest {
    /// Emit dynamic completion registration for one shell.
    GenerateCompletions(clap_complete::Shell),
    /// Explicitly refresh workspace-aware completion candidates.
    RefreshCompletions(DiscoveryRequest),
    /// Generate a project from a catalog or direct local template.
    Init(InitRequest),
    /// List effective project templates.
    Templates {
        /// Explicit catalog file.
        catalog_path: Option<PathBuf>,
    },
    /// Check or doctor report.
    Report {
        command: CommandKind,
        discovery: DiscoveryRequest,
        format: String,
    },
    /// Human or canonical JSON inspection.
    Inspect {
        discovery: DiscoveryRequest,
        format: String,
        filters: InspectFilters,
        color: TerminalPolicy,
        pager: TerminalPolicy,
    },
    /// Canonical document or envelope export.
    Export {
        discovery: DiscoveryRequest,
        format: String,
        output: Option<PathBuf>,
    },
    /// Deterministic graph projection.
    Graph {
        discovery: DiscoveryRequest,
        format: String,
        query: GraphQuery,
        color: TerminalPolicy,
        pager: TerminalPolicy,
    },
    /// One deterministic resource explanation.
    Explain {
        discovery: DiscoveryRequest,
        format: String,
        resource: String,
        preview: Option<TransitionPreviewRequest>,
        color: TerminalPolicy,
        pager: TerminalPolicy,
    },
}

impl Cli {
    pub(crate) fn parse_cargo() -> Self {
        let arguments = normalized_arguments(std::env::args_os());
        let mut matches = Self::command()
            .color(help_color_policy())
            .get_matches_from(arguments);

        Self::from_arg_matches_mut(&mut matches).expect("Clap arguments match the derived CLI")
    }

    pub(crate) fn into_request(self) -> CommandRequest {
        match self.command {
            Command::Completions(arguments) => match arguments.command {
                CompletionsCommand::Generate { shell } => {
                    CommandRequest::GenerateCompletions(shell)
                }
                CompletionsCommand::Refresh { target } => {
                    CommandRequest::RefreshCompletions(discovery_request(target))
                }
            },
            Command::Init(arguments) => {
                let template = arguments.template_path.map_or_else(
                    || TemplateSelection::Catalog {
                        template: arguments.template,
                        catalog_path: arguments.catalog,
                    },
                    TemplateSelection::Local,
                );

                CommandRequest::Init(InitRequest {
                    destination: arguments.path,
                    name: arguments.name,
                    template,
                    workspace: arguments.workspace,
                    no_vcs: arguments.no_vcs,
                    define: arguments.define,
                    upwell_path: arguments.upwell_path,
                })
            }
            Command::Templates(arguments) => CommandRequest::Templates {
                catalog_path: arguments.catalog,
            },
            Command::Check(arguments) => CommandRequest::Report {
                command: CommandKind::Check,
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
            },
            Command::Doctor(arguments) => CommandRequest::Report {
                command: CommandKind::Doctor,
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
            },
            Command::Inspect(arguments) => CommandRequest::Inspect {
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
                filters: arguments.filters,
                color: arguments.terminal.color,
                pager: arguments.terminal.pager,
            },
            Command::Export(arguments) => CommandRequest::Export {
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
                output: arguments.output,
            },
            Command::Graph(arguments) => CommandRequest::Graph {
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
                query: GraphQuery {
                    resources: arguments.resources,
                    contributors: arguments.contributors,
                    plugins: arguments.plugins,
                    family: arguments.family.into(),
                    direction: arguments.direction.into(),
                },
                color: arguments.terminal.color,
                pager: arguments.terminal.pager,
            },
            Command::Explain(arguments) => CommandRequest::Explain {
                discovery: discovery_request(arguments.target),
                format: arguments.format.into_value(),
                resource: arguments.resource,
                preview: (!arguments.set.is_empty()).then_some(TransitionPreviewRequest {
                    assignments: arguments.set,
                }),
                color: arguments.terminal.color,
                pager: arguments.terminal.pager,
            },
        }
    }
}

pub(crate) fn completion_command() -> clap::Command {
    Cli::command().bin_name("cargo-upwell")
}

fn complete_templates(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let mut candidates = Catalog::builtins()
        .templates()
        .filter(|template| template.id().starts_with(prefix.as_ref()))
        .map(|template| {
            CompletionCandidate::new(template.id())
                .help(Some(template.description().to_owned().into()))
        })
        .collect::<Vec<_>>();

    candidates.extend(complete_cached(
        completion::CandidateKind::Template,
        current,
    ));
    candidates.sort_by(|left, right| left.get_value().cmp(right.get_value()));
    candidates.dedup_by(|left, right| left.get_value() == right.get_value());
    candidates
}

fn complete_packages(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Package, current)
}

fn complete_binaries(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Binary, current)
}

fn complete_features(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Feature, current)
}

fn complete_resources(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Resource, current)
}

fn complete_contributors(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Contributor, current)
}

fn complete_plugins(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Plugin, current)
}

fn complete_scopes(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Scope, current)
}

fn complete_facets(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    complete_cached(completion::CandidateKind::Facet, current)
}

fn complete_cached(
    kind: completion::CandidateKind,
    current: &std::ffi::OsStr,
) -> Vec<CompletionCandidate> {
    let Ok(directory) = std::env::current_dir() else {
        return Vec::new();
    };
    let prefix = current.to_string_lossy();

    completion::candidates(kind, &directory)
        .into_iter()
        .filter(|candidate| candidate.value.starts_with(prefix.as_ref()))
        .map(|candidate| {
            CompletionCandidate::new(candidate.value).help(candidate.help.map(Into::into))
        })
        .collect()
}

fn help_color_policy() -> ColorChoice {
    match std::env::var("CARGO_TERM_COLOR").as_deref() {
        Ok("always") => ColorChoice::Always,
        Ok("never") => ColorChoice::Never,
        _ if std::env::var_os("NO_COLOR").is_some() => ColorChoice::Never,
        _ if std::env::var_os("TERM").is_some_and(|value| value == "dumb") => ColorChoice::Never,
        _ => ColorChoice::Auto,
    }
}

fn discovery_request(arguments: TargetArgs) -> DiscoveryRequest {
    let current_dir = std::env::current_dir().ok();

    DiscoveryRequest {
        cargo: CargoExecutable::from_environment(),
        manifest_path: arguments.manifest_path,
        current_dir,
        package: arguments.package,
        binary: arguments.binary,
        features: FeatureSelection {
            no_default_features: arguments.no_default_features,
            all_features: arguments.all_features,
            features: arguments.features,
        },
        target: arguments.target,
    }
}

fn normalized_arguments(arguments: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut arguments = arguments.into_iter().collect::<Vec<_>>();

    if arguments
        .get(1)
        .is_some_and(|argument| argument == "upwell")
    {
        arguments.remove(1);
    }

    arguments
}

#[cfg(test)]
mod tests;
