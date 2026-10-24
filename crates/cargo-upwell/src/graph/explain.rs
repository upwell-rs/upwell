use std::collections::BTreeSet;

use upwell_tooling_schema::{
    CliArgument, CliCommand, CliOwner, CliProvider, Provenance, Resource, ResourceKind,
    ToolingDocument,
};

use crate::TOOLING_SCHEMA_VERSION;

use super::error::{GraphQueryError, GraphSelectorKind};
use super::index::GraphIndex;
use super::model::{
    CliArgumentOwnership, CliCommandOwnership, CliOwnershipSummary, ResourceExplanation,
};
use super::query::relevant_diagnostics;
use super::selection::provider_selection;

/// Explains one exact stable resource ID or exact case-sensitive unique resource name.
pub fn explain_resource(
    document: &ToolingDocument,
    selector: &str,
) -> Result<ResourceExplanation, GraphQueryError> {
    let index = GraphIndex::new(document)?;
    let id = index.resolve(selector, GraphSelectorKind::Resource, |_| true)?;
    let selected = BTreeSet::from([id.clone()]);
    let resource = index.resource(&id).clone();
    let incoming = index.relationships_for(&id, false);
    let outgoing = index.relationships_for(&id, true);
    let diagnostics = relevant_diagnostics(&index.document.diagnostics, &selected);
    let cli_providers = index.relevant_cli_providers(&selected);
    let cli_ownership = index.cli_ownership(&resource, &cli_providers);
    let selection = provider_selection(&index, &resource);
    let transition = index.document.transition_preview.clone();

    Ok(ResourceExplanation {
        schema: TOOLING_SCHEMA_VERSION,
        source: index.source(),
        resource,
        incoming,
        outgoing,
        diagnostics,
        cli_providers,
        cli_ownership,
        selection,
        transition,
    })
}

pub(super) fn cli_ownership(
    document: &ToolingDocument,
    resource: &Resource,
    providers: &[CliProvider],
) -> CliOwnershipSummary {
    let Some(cli) = &document.cli else {
        return CliOwnershipSummary::default();
    };
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect::<BTreeSet<_>>();
    let include_application = resource.kind == ResourceKind::Application;
    let include_framework = is_framework_owner(resource.provenance.as_ref());
    let mut summary = CliOwnershipSummary::default();
    let mut path = Vec::new();

    collect_cli_ownership(
        &cli.root,
        &mut path,
        &provider_ids,
        include_application,
        include_framework,
        &mut summary,
    );

    summary
}

fn collect_cli_ownership(
    command: &CliCommand,
    path: &mut Vec<String>,
    providers: &BTreeSet<&str>,
    include_application: bool,
    include_framework: bool,
    summary: &mut CliOwnershipSummary,
) {
    path.push(command.name.clone());

    if owner_matches(
        &command.owner,
        providers,
        include_application,
        include_framework,
    ) {
        summary.commands.push(CliCommandOwnership {
            path: path.clone(),
            id: command.id.clone(),
            owner: command.owner.clone(),
        });
    }

    for argument in &command.arguments {
        collect_cli_argument(
            argument,
            path,
            providers,
            include_application,
            include_framework,
            summary,
        );
    }

    for child in &command.commands {
        collect_cli_ownership(
            child,
            path,
            providers,
            include_application,
            include_framework,
            summary,
        );
    }

    path.pop();
}

fn collect_cli_argument(
    argument: &CliArgument,
    command_path: &[String],
    providers: &BTreeSet<&str>,
    include_application: bool,
    include_framework: bool,
    summary: &mut CliOwnershipSummary,
) {
    if owner_matches(
        &argument.owner,
        providers,
        include_application,
        include_framework,
    ) {
        summary.arguments.push(CliArgumentOwnership {
            command_path: command_path.to_vec(),
            id: argument.id.clone(),
            owner: argument.owner.clone(),
        });
    }
}

fn owner_matches(
    owner: &CliOwner,
    providers: &BTreeSet<&str>,
    include_application: bool,
    include_framework: bool,
) -> bool {
    match owner {
        CliOwner::Framework => include_framework,
        CliOwner::Application => include_application,
        CliOwner::Plugin { provider } => providers.contains(provider.as_str()),
    }
}

fn is_framework_owner(provenance: Option<&Provenance>) -> bool {
    provenance.and_then(|provenance| provenance.origin.as_deref()) == Some("framework")
}
