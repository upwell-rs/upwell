//! Generated-CLI metadata and contribution identities.

use serde::{Deserialize, Serialize};

use crate::is_blank;

mod validate;

pub(crate) use validate::*;

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
