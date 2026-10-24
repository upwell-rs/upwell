//! Canonicalization and validation of generated-CLI metadata.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CliCommand, CliMetadata, CliOwner, Resource, ValidationError, allowed_provenance_self_owner,
    is_blank, parse_cli_provider_id,
};

pub(crate) fn canonicalize_cli(cli: &mut CliMetadata) {
    cli.providers.sort_by(|left, right| left.id.cmp(&right.id));
    canonicalize_cli_command(&mut cli.root);
}

fn canonicalize_cli_command(command: &mut CliCommand) {
    sort_dedup(&mut command.aliases);
    sort_dedup(&mut command.visible_aliases);
    sort_dedup(&mut command.short_flag_aliases);
    sort_dedup(&mut command.visible_short_flag_aliases);
    sort_dedup(&mut command.long_flag_aliases);
    sort_dedup(&mut command.visible_long_flag_aliases);

    for argument in &mut command.arguments {
        sort_dedup(&mut argument.aliases);
        sort_dedup(&mut argument.visible_aliases);
        sort_dedup(&mut argument.short_aliases);
        sort_dedup(&mut argument.visible_short_aliases);
    }

    for child in &mut command.commands {
        canonicalize_cli_command(child);
    }

    command.arguments.sort_by(|left, right| {
        (left.index.is_none(), left.index, &left.id).cmp(&(
            right.index.is_none(),
            right.index,
            &right.id,
        ))
    });
    command
        .commands
        .sort_by(|left, right| left.name.cmp(&right.name));
}

pub(crate) fn sort_dedup<T: Ord>(values: &mut Vec<T>) {
    values.sort();
    values.dedup();
}

pub(crate) fn validate_cli(
    cli: &CliMetadata,
    resources: &BTreeMap<&str, &Resource>,
) -> Result<(), ValidationError> {
    let mut providers = BTreeSet::new();
    let mut command_ids = BTreeSet::new();

    for provider in &cli.providers {
        if is_blank(&provider.id) {
            return Err(ValidationError::EmptyCliProviderId);
        }

        if is_blank(&provider.contributor) {
            return Err(ValidationError::EmptyCliProviderContributor {
                id: provider.id.clone(),
            });
        }

        if is_blank(&provider.contribution) {
            return Err(ValidationError::EmptyCliProviderContribution {
                id: provider.id.clone(),
            });
        }

        let identity = parse_cli_provider_id(&provider.id);

        if identity.is_none_or(|identity| {
            identity.contributor() != provider.contributor
                || identity.contribution() != provider.contribution
        }) {
            return Err(ValidationError::InvalidCliProviderIdentity {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
                contribution: provider.contribution.clone(),
            });
        }

        let Some(contributor) = resources.get(provider.contributor.as_str()) else {
            return Err(ValidationError::InvalidCliProviderContributor {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
            });
        };

        if !allowed_provenance_self_owner(&contributor.kind) {
            return Err(ValidationError::InvalidCliProviderContributor {
                id: provider.id.clone(),
                contributor: provider.contributor.clone(),
            });
        }

        if !providers.insert(provider.id.as_str()) {
            return Err(ValidationError::DuplicateCliProvider {
                id: provider.id.clone(),
            });
        }
    }

    validate_cli_command(
        &cli.root,
        "",
        &providers,
        &CliOptionNamespace::default(),
        &mut command_ids,
    )?;

    if let Some(default_command) = &cli.default_command
        && (is_blank(default_command) || !command_ids.contains(default_command))
    {
        return Err(ValidationError::UnknownDefaultCliCommand {
            id: default_command.clone(),
        });
    }

    Ok(())
}

#[derive(Clone, Default)]
struct CliOptionNamespace {
    long: BTreeSet<String>,
    short: BTreeSet<char>,
}

fn validate_cli_command(
    command: &CliCommand,
    parent: &str,
    providers: &BTreeSet<&str>,
    inherited: &CliOptionNamespace,
    command_ids: &mut BTreeSet<String>,
) -> Result<(), ValidationError> {
    if is_blank(&command.name) {
        return Err(ValidationError::EmptyCliCommandName {
            path: parent.to_string(),
        });
    }

    let path = if parent.is_empty() {
        command.name.clone()
    } else {
        format!("{parent} {}", command.name)
    };
    let mut arguments = BTreeSet::new();
    let mut positions = BTreeSet::new();
    let mut commands = BTreeSet::new();
    let mut options = inherited.clone();
    let mut globals = inherited.clone();

    validate_cli_owner(&command.owner, &path, providers)?;

    if let Some(id) = &command.id {
        if is_blank(id) {
            return Err(ValidationError::EmptyCliCommandId { path });
        }

        if !command_ids.insert(id.clone()) {
            return Err(ValidationError::DuplicateCliCommandId { id: id.clone() });
        }
    }

    for argument in &command.arguments {
        if is_blank(&argument.id) {
            return Err(ValidationError::EmptyCliArgumentId { path });
        }

        if !arguments.insert(argument.id.as_str()) {
            return Err(ValidationError::DuplicateCliArgument {
                path,
                id: argument.id.clone(),
            });
        }

        if let Some(index) = argument.index {
            if index == 0 {
                return Err(ValidationError::InvalidCliPosition {
                    path,
                    id: argument.id.clone(),
                });
            }

            if !positions.insert(index) {
                return Err(ValidationError::DuplicateCliPosition { path, index });
            }
        }

        if let Some(max) = argument.cardinality.max_values
            && max < argument.cardinality.min_values
        {
            return Err(ValidationError::InvalidCliCardinality {
                path,
                id: argument.id.clone(),
                min: argument.cardinality.min_values,
                max,
            });
        }

        validate_cli_owner(
            &argument.owner,
            &format!("{path}::{}", argument.id),
            providers,
        )?;

        for long in argument
            .long
            .iter()
            .chain(&argument.aliases)
            .chain(&argument.visible_aliases)
        {
            insert_cli_long(&mut options, &path, long)?;

            if argument.global {
                insert_cli_long_if_absent(&mut globals, long);
            }
        }

        for short in argument
            .short
            .iter()
            .chain(&argument.short_aliases)
            .chain(&argument.visible_short_aliases)
        {
            insert_cli_short(&mut options, &path, *short)?;

            if argument.global {
                globals.short.insert(*short);
            }
        }
    }

    for child in &command.commands {
        for name in std::iter::once(&child.name)
            .chain(&child.aliases)
            .chain(&child.visible_aliases)
        {
            if is_blank(name) {
                return Err(ValidationError::EmptyCliParserName {
                    path,
                    kind: "command name or alias",
                });
            }

            if !commands.insert(name.as_str()) {
                return Err(ValidationError::DuplicateCliCommand {
                    path,
                    name: name.clone(),
                });
            }
        }

        for long in child
            .long_flag
            .iter()
            .chain(&child.long_flag_aliases)
            .chain(&child.visible_long_flag_aliases)
        {
            insert_cli_long(&mut options, &path, long)?;
        }

        for short in child
            .short_flag
            .iter()
            .chain(&child.short_flag_aliases)
            .chain(&child.visible_short_flag_aliases)
        {
            insert_cli_short(&mut options, &path, *short)?;
        }

        validate_cli_command(child, &path, providers, &globals, command_ids)?;
    }

    Ok(())
}

fn insert_cli_long(
    namespace: &mut CliOptionNamespace,
    path: &str,
    name: &str,
) -> Result<(), ValidationError> {
    if is_blank(name) {
        return Err(ValidationError::EmptyCliParserName {
            path: path.to_string(),
            kind: "long option",
        });
    }

    if !namespace.long.insert(name.to_string()) {
        return Err(ValidationError::DuplicateCliLongOption {
            path: path.to_string(),
            name: name.to_string(),
        });
    }

    Ok(())
}

fn insert_cli_long_if_absent(namespace: &mut CliOptionNamespace, name: &str) {
    namespace.long.insert(name.to_string());
}

fn insert_cli_short(
    namespace: &mut CliOptionNamespace,
    path: &str,
    name: char,
) -> Result<(), ValidationError> {
    if name.is_whitespace() {
        return Err(ValidationError::EmptyCliParserName {
            path: path.to_string(),
            kind: "short option",
        });
    }

    if !namespace.short.insert(name) {
        return Err(ValidationError::DuplicateCliShortOption {
            path: path.to_string(),
            name,
        });
    }

    Ok(())
}

fn validate_cli_owner(
    owner: &CliOwner,
    path: &str,
    providers: &BTreeSet<&str>,
) -> Result<(), ValidationError> {
    let CliOwner::Plugin { provider } = owner else {
        return Ok(());
    };

    if !providers.contains(provider.as_str()) {
        return Err(ValidationError::UnknownCliProvider {
            path: path.to_string(),
            id: provider.clone(),
        });
    }

    Ok(())
}
