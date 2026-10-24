use std::ffi::OsStr;
use std::marker::PhantomData;

use cargo_upwell::{GraphDirection, GraphRelationFamily, RendererCommand};
use clap::builder::{PossibleValue, TypedValueParser};
use clap::{Args, ValueEnum};
use clap_complete::engine::ValueCompleter;
use clap_complete::{ArgValueCompleter, CompletionCandidate};

pub(super) trait FormatCommand: Clone + Send + Sync + 'static {
    const COMMAND: RendererCommand;
}

#[derive(Clone, Debug, Args)]
pub(crate) struct Format<C: FormatCommand> {
    /// Output representation.
    #[arg(long = "format", value_name = "FORMAT", help = "Output representation (shell completion also includes configured formats)", value_parser = FormatParser::<C>::new(), add = ArgValueCompleter::new(FormatCompleter::<C>::new()))]
    value: Option<String>,
    #[arg(skip)]
    command: PhantomData<C>,
}

impl<C: FormatCommand> Format<C> {
    pub(crate) fn into_value(self) -> String {
        self.value.unwrap_or_else(|| {
            crate::render_runtime::format_registry()
                .default_format(C::COMMAND)
                .to_owned()
        })
    }
}

#[cfg(test)]
pub(crate) fn format_candidates<C: FormatCommand>(current: &OsStr) -> Vec<CompletionCandidate> {
    FormatCompleter::<C>::new().complete(current)
}

#[derive(Clone, Copy, Debug)]
struct FormatParser<C>(PhantomData<C>);

impl<C> FormatParser<C> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: FormatCommand> TypedValueParser for FormatParser<C> {
    type Value = String;

    fn parse_ref(
        &self,
        command: &clap::Command,
        argument: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let Some(value) = value.to_str() else {
            return Err(clap::Error::new(clap::error::ErrorKind::InvalidUtf8).with_cmd(command));
        };
        let registry = crate::render_runtime::format_registry();

        if registry.resolve(C::COMMAND, value).is_some() {
            return Ok(value.to_owned());
        }

        let available = registry
            .formats(C::COMMAND)
            .map(|renderer| renderer.format().id())
            .collect::<Vec<_>>()
            .join(", ");
        let argument = argument
            .map(|argument| argument.to_string())
            .unwrap_or_else(|| String::from("format"));

        Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            format!(
                "invalid value `{value}` for {argument}: unknown {} format; available formats: {available}",
                C::COMMAND
            ),
        )
        .with_cmd(command))
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = crate::render_runtime::format_registry()
            .formats(C::COMMAND)
            .map(|renderer| PossibleValue::new(renderer.format().id()))
            .collect::<Vec<_>>();

        Some(Box::new(values.into_iter()))
    }
}

#[derive(Clone, Copy, Debug)]
struct FormatCompleter<C>(PhantomData<C>);

impl<C> FormatCompleter<C> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: FormatCommand> ValueCompleter for FormatCompleter<C> {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        let prefix = current.to_string_lossy();
        let registry = crate::render_runtime::format_registry();

        registry
            .formats(C::COMMAND)
            .filter(|renderer| renderer.format().id().starts_with(prefix.as_ref()))
            .map(|renderer| {
                CompletionCandidate::new(renderer.format().id()).help(Some(
                    format!(
                        "{} ({})",
                        renderer.descriptor().id(),
                        renderer.format().media_type()
                    )
                    .into(),
                ))
            })
            .collect()
    }
}

macro_rules! format_command {
    ($name:ident, $command:ident) => {
        #[derive(Clone, Copy, Debug)]
        pub(crate) struct $name;

        impl FormatCommand for $name {
            const COMMAND: RendererCommand = RendererCommand::$command;
        }
    };
}

format_command!(Check, Check);
format_command!(Doctor, Doctor);
format_command!(Inspect, Inspect);
format_command!(Export, Export);
format_command!(Graph, Graph);
format_command!(Explain, Explain);

/// Supported check and doctor output representations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum ReportFormat {
    /// Human-readable terminal output.
    #[default]
    Terminal,
    /// Versioned machine-readable JSON.
    Json,
}

/// Supported inspection output representations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum InspectFormat {
    /// Human-readable generic inspection.
    #[default]
    Text,
    /// Canonical tooling document JSON.
    Json,
}

/// Supported graph output representations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum GraphFormat {
    /// Human-readable terminal list.
    #[default]
    Text,
    /// Mermaid flowchart source.
    Mermaid,
    /// Graphviz DOT source.
    Dot,
    /// Canonical graph-view JSON.
    Json,
}

/// Supported explanation output representations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum ExplainFormat {
    /// Human-readable terminal explanation.
    #[default]
    Text,
    /// Canonical resource-explanation JSON.
    Json,
}

/// Semantic graph relationship family accepted by the CLI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(super) enum GraphFamily {
    #[default]
    All,
    Dependencies,
    Composition,
    Scopes,
    Lifecycle,
    Ownership,
}

impl From<GraphFamily> for GraphRelationFamily {
    fn from(family: GraphFamily) -> Self {
        match family {
            GraphFamily::All => Self::All,
            GraphFamily::Dependencies => Self::Dependencies,
            GraphFamily::Composition => Self::Composition,
            GraphFamily::Scopes => Self::Scopes,
            GraphFamily::Lifecycle => Self::Lifecycle,
            GraphFamily::Ownership => Self::Ownership,
        }
    }
}

/// Semantic graph traversal direction accepted by the CLI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(super) enum GraphTraversalDirection {
    #[default]
    Both,
    Upstream,
    Downstream,
}

impl From<GraphTraversalDirection> for GraphDirection {
    fn from(direction: GraphTraversalDirection) -> Self {
        match direction {
            GraphTraversalDirection::Both => Self::Both,
            GraphTraversalDirection::Upstream => Self::Upstream,
            GraphTraversalDirection::Downstream => Self::Downstream,
        }
    }
}

/// Automatic, forced, or disabled terminal behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum TerminalPolicy {
    /// Enable behavior only for an interactive terminal.
    #[default]
    Auto,
    /// Always enable behavior.
    Always,
    /// Never enable behavior.
    Never,
}

/// Generic resource kind accepted by inspection filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum InspectResourceKind {
    Application,
    Protocol,
    Plugin,
    Component,
    Provider,
    ConfigBinding,
    Hook,
    Lifecycle,
    Scope,
    Type,
    Contribution,
    Contributor,
    PluginSlot,
    Condition,
}

/// CLI provider kind accepted by inspection filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum InspectCliProviderKind {
    Args,
    Command,
    CommandSet,
}
