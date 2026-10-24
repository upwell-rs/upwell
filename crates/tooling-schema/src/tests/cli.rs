use super::*;

#[test]
fn contribution_and_cli_provider_identities_round_trip_canonically() {
    let contributor = "plugin:test/worker";
    let contribution = "test/worker-cli";
    let resource = contribution_id(contributor, contribution);
    let provider = cli_provider_id(contributor, contribution);

    for (identity, parsed) in [
        (resource.as_str(), parse_contribution_id(&resource)),
        (provider.as_str(), parse_cli_provider_id(&provider)),
    ] {
        let parsed = parsed.expect("canonical identity parses");

        assert_eq!(parsed.contributor(), contributor, "{identity}");
        assert_eq!(parsed.contribution(), contribution, "{identity}");
    }

    assert!(parse_contribution_id(&provider).is_none());
    assert!(parse_cli_provider_id(&resource).is_none());
    assert!(parse_cli_provider_id("cli-provider:plugin:test/worker:").is_none());
}

#[test]
fn typed_cli_round_trips_aliases_cardinality_nesting_and_provenance() {
    let mut document = fixture();

    add_cli_resources(&mut document);
    document.cli = Some(cli_fixture());

    let json = document
        .to_canonical_json()
        .expect("typed CLI document emits");
    let decoded: ToolingDocument = serde_json::from_str(&json).expect("typed CLI document decodes");
    let cli = decoded.cli.expect("CLI section exists");
    let input = &cli.root.arguments[0];

    assert_eq!(input.aliases, ["a", "z"]);
    assert_eq!(input.visible_aliases, ["visible"]);
    assert_eq!(input.value_names, ["FIRST", "REST"]);
    assert!(input.default_values.is_empty());
    assert_eq!(input.cardinality.max_values, None);
    assert!(input.cardinality.repeatable);
    assert!(matches!(cli.root.arguments[1].owner, CliOwner::Framework));
    assert!(json.contains("\"owner\":{\"kind\":\"framework\"}"));
    assert!(!json.contains("\"slot\""));
    assert_eq!(cli.root.commands[0].name, "inspect");
    assert!(matches!(input.owner, CliOwner::Plugin { .. }));
}

#[test]
fn typed_cli_provider_and_declaration_permutations_canonicalize_identically() {
    let mut left = fixture();
    let mut right = fixture();
    let mut cli = cli_fixture();
    let second = CliProvider {
        id: String::from("cli-provider:plugin:test/plugin:test/commands"),
        contributor: String::from("plugin:test/plugin"),
        contribution: String::from("test/commands"),
        kind: CliProviderKind::CommandSet,
    };

    cli.providers.push(second);
    cli.root.commands.push(CliCommand {
        name: String::from("admin"),
        owner: CliOwner::Application,
        ..CliCommand::default()
    });
    add_cli_resources(&mut left);
    add_cli_resources(&mut right);
    left.cli = Some(cli.clone());
    cli.providers.reverse();
    cli.root.commands.reverse();
    cli.root.arguments[0].aliases.reverse();
    right.cli = Some(cli);

    assert_eq!(
        left.to_canonical_json().expect("left emits"),
        right.to_canonical_json().expect("right emits")
    );
}

#[test]
fn typed_cli_validation_rejects_unknown_provider_and_invalid_cardinality() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.arguments[0].cardinality = CliCardinality {
        min_values: 2,
        max_values: Some(1),
        repeatable: false,
    };
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliCardinality { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers.clear();
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::UnknownCliProvider { .. })
    ));
}

#[test]
fn typed_cli_validation_rejects_duplicate_provider_identities() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.providers.push(cli.providers[0].clone());
    document.cli = Some(cli);

    assert!(matches!(
        document.to_canonical_json(),
        Err(crate::EmitError::Validation(
            ValidationError::DuplicateCliProvider { .. }
        ))
    ));
}

#[test]
fn cli_provider_identity_contributor_and_resource_must_correspond() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.providers[0].contribution.clear();
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliProviderContribution { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers[0].contributor = String::from("plugin:test/missing");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliProviderIdentity { .. })
    ));

    let mut cli = cli_fixture();

    cli.providers[0].id = String::from("cli-provider:plugin:test/missing:test/args");
    cli.providers[0].contributor = String::from("plugin:test/missing");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliProviderContributor { .. })
    ));
}

#[test]
fn typed_cli_validation_rejects_blank_identities_and_parser_namespace_collisions() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.name = String::from(" \t");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliCommandName { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[0].id = String::from(" ");
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliArgumentId { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[1].short = Some(' ');
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::EmptyCliParserName { .. })
    ));

    for (argument, expected) in [
        (
            CliArgument {
                id: String::from("long-collision"),
                long: Some(String::from("profile")),
                owner: CliOwner::Application,
                ..CliArgument::default()
            },
            "long",
        ),
        (
            CliArgument {
                id: String::from("short-collision"),
                short: Some('p'),
                owner: CliOwner::Application,
                ..CliArgument::default()
            },
            "short",
        ),
    ] {
        let mut cli = cli_fixture();

        cli.root.arguments[1].short = Some('p');
        cli.root.arguments.push(argument);
        document.cli = Some(cli);

        match (expected, document.validate()) {
            ("long", Err(ValidationError::DuplicateCliLongOption { .. }))
            | ("short", Err(ValidationError::DuplicateCliShortOption { .. })) => {}
            (_, result) => panic!("unexpected namespace validation result: {result:?}"),
        }
    }
}

#[test]
fn typed_cli_validation_checks_aliases_inherited_globals_and_default_command_ids() {
    let mut document = fixture();

    add_cli_resources(&mut document);

    let mut cli = cli_fixture();

    cli.root.commands[0].aliases = vec![String::from("duplicate")];
    cli.root.commands.push(CliCommand {
        name: String::from("other"),
        visible_aliases: vec![String::from("duplicate")],
        owner: CliOwner::Application,
        ..CliCommand::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::DuplicateCliCommand { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.arguments[1].global = true;
    cli.root.commands[0].arguments.push(CliArgument {
        id: String::from("nested-profile"),
        visible_aliases: vec![String::from("profile")],
        owner: CliOwner::Application,
        ..CliArgument::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::DuplicateCliLongOption { .. })
    ));

    let mut cli = cli_fixture();

    cli.default_command = Some(String::from("serve"));
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::UnknownDefaultCliCommand { .. })
    ));

    let mut cli = cli_fixture();

    cli.root.commands[0].id = Some(String::from("serve"));
    cli.default_command = Some(String::from("serve"));
    document.cli = Some(cli);

    document
        .validate()
        .expect("canonical default command resolves independently of its parser name");
}

#[test]
fn typed_cli_validation_rejects_zero_positional_index() {
    let mut document = fixture();
    let mut cli = cli_fixture();

    add_cli_resources(&mut document);
    cli.root.arguments.push(CliArgument {
        id: String::from("invalid-position"),
        index: Some(0),
        owner: CliOwner::Application,
        ..CliArgument::default()
    });
    document.cli = Some(cli);

    assert!(matches!(
        document.validate(),
        Err(ValidationError::InvalidCliPosition { .. })
    ));
}
