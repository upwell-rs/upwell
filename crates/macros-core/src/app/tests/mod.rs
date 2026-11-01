use std::str::FromStr as _;

use proc_macro2::{Delimiter, LineColumn, Span, TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::parse2;

use super::{NamedApp, expand};

fn parse_error(input: proc_macro2::TokenStream) -> String {
    match parse2::<NamedApp>(input) {
        Ok(_) => panic!("input unexpectedly parsed"),
        Err(error) => error.to_string(),
    }
}

fn parse_error_with_span(input: &str) -> syn::Error {
    let input = TokenStream::from_str(input).expect("test token stream parses");

    match parse2::<NamedApp>(input) {
        Ok(_) => panic!("input unexpectedly parsed"),
        Err(error) => error,
    }
}

fn identifier_span(input: &str, identifier: &str, occurrence: usize) -> Span {
    let tokens = TokenStream::from_str(input).expect("test token stream parses");
    let mut spans = Vec::new();

    collect_identifier_spans(tokens, identifier, &mut spans);

    spans[occurrence]
}

fn group_span(input: &str, delimiter: Delimiter, occurrence: usize) -> Span {
    let tokens = TokenStream::from_str(input).expect("test token stream parses");
    let mut spans = Vec::new();

    collect_group_spans(tokens, delimiter, &mut spans);

    spans[occurrence]
}

fn collect_identifier_spans(tokens: TokenStream, identifier: &str, spans: &mut Vec<Span>) {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) if ident == identifier => spans.push(ident.span()),
            TokenTree::Group(group) => collect_identifier_spans(group.stream(), identifier, spans),
            _ => {}
        }
    }
}

fn collect_group_spans(tokens: TokenStream, delimiter: Delimiter, spans: &mut Vec<Span>) {
    for token in tokens {
        if let TokenTree::Group(group) = token {
            if group.delimiter() == delimiter {
                spans.push(group.span());
            }

            collect_group_spans(group.stream(), delimiter, spans);
        }
    }
}

fn assert_same_location(actual: Span, expected: Span) {
    assert_eq!(actual.start(), expected.start());
    assert_eq!(actual.end(), expected.end());
}

fn assert_semantic_ident(actual: &syn::Ident, spelling: &str, expected: Span, source: &str) {
    assert_eq!(actual, spelling);
    assert_eq!(actual.span().byte_range(), expected.byte_range());
    assert_eq!(actual.span().start(), expected.start());
    assert_eq!(actual.span().end(), expected.end());

    if let Some(source_text) = actual.span().source_text() {
        assert_eq!(source_text, source);
    }
}

#[cfg(feature = "cli")]
fn assert_not_source_anchored(actual: &syn::Ident, expected: Span, source: &str) {
    assert_ne!(actual.span().byte_range(), expected.byte_range());

    if let Some(source_text) = actual.span().source_text() {
        assert_ne!(source_text, source);
    }
}

fn app_host_impl(file: &syn::File) -> &syn::ItemImpl {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item)
                if item.trait_.as_ref().is_some_and(|(_, path, _)| {
                    path.segments
                        .last()
                        .is_some_and(|segment| segment.ident == "AppHost")
                }) =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("expansion contains AppHost impl")
}

fn impl_method<'a>(item: &'a syn::ItemImpl, name: &str) -> &'a syn::ImplItemFn {
    item.items
        .iter()
        .find_map(|item| match item {
            syn::ImplItem::Fn(item) if item.sig.ident == name => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("AppHost impl contains `{name}`"))
}

#[cfg(feature = "cli")]
fn named_struct<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemStruct {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == name => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expansion contains struct `{name}`"))
}

#[cfg(feature = "cli")]
fn named_enum<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemEnum {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == name => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expansion contains enum `{name}`"))
}

#[cfg(feature = "cli")]
fn named_field<'a>(item: &'a syn::ItemStruct, name: &str) -> &'a syn::Field {
    item.fields
        .iter()
        .find(|field| field.ident.as_ref().is_some_and(|ident| ident == name))
        .unwrap_or_else(|| panic!("struct contains field `{name}`"))
}

#[cfg(feature = "cli")]
fn named_variant<'a>(item: &'a syn::ItemEnum, name: &str) -> &'a syn::Variant {
    item.variants
        .iter()
        .find(|variant| variant.ident == name)
        .unwrap_or_else(|| panic!("enum contains variant `{name}`"))
}

fn expanded_identifier_locations(output: &TokenStream, identifier: &str) -> Vec<LineColumn> {
    let mut spans = Vec::new();

    collect_identifier_spans(output.clone(), identifier, &mut spans);

    spans.into_iter().map(|span| span.start()).collect()
}

#[test]
fn parses_named_app_visibilities() {
    for input in [
        quote!(app Private { name: "private", protocol: Protocol }),
        quote!(pub app Public { name: "public", protocol: Protocol }),
        quote!(pub(crate) app CrateOnly { name: "crate", protocol: Protocol }),
    ] {
        parse2::<NamedApp>(input).expect("named app parses");
    }
}

#[test]
fn rejects_non_documentation_attributes_on_named_apps() {
    let error = match parse2::<NamedApp>(quote! {
        #[cfg(feature = "disabled")]
        app Example {
            name: "example",
            protocol: Protocol,
        }
    }) {
        Ok(_) => panic!("partial generated cfg attributes unexpectedly parsed"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("only documentation attributes are supported on generated applications")
    );
}

#[test]
fn parses_complete_named_app() {
    let input = quote! {
        pub app Example {
            name: "example",
            protocol: Protocol,
            services: [Service],
            components: [component()],
            configs: [Config => "app.config"],
            managers: {
                directories: directories(),
                config: config(),
            },
            middleware: [middleware()],
            guards: [guard()],
            error_handler: error_handler(),
            deny_unplanned: true,
            plugins: [Plugin, replace SLOT => Replacement, suppress OPTIONAL_SLOT],
            upwell: ::framework,
        }
    };

    parse2::<NamedApp>(input).expect("complete named app parses");
}

#[test]
fn rejects_inert_app_crate_override_as_unknown() {
    let input = "app Example { name: \"example\", protocol: Protocol, crate: ::plugin }";
    let error = parse_error_with_span(input);
    let krate = identifier_span(input, "crate", 0);

    assert!(error.to_string().contains("unknown `app!` key `crate`"));
    assert_same_location(error.span(), krate);
}

#[test]
fn unknown_app_key_lists_lifecycle_keys() {
    let error = parse_error(quote! {
        app Example {
            name: "example",
            protocol: Protocol,
            unknown: true,
        }
    });

    for key in ["setup", "configure", "before_build", "after_build", "serve"] {
        assert!(error.contains(key));
    }
}

#[test]
fn manager_validation_uses_offending_key_or_block_spans() {
    let duplicate = "app Example {
        name: \"example\",
        protocol: Protocol,
        managers: { config: { source: first(), source: second() } },
    }";
    let error = parse_error_with_span(duplicate);

    assert!(
        error
            .to_string()
            .contains("duplicate `config` setting `source`")
    );
    assert_same_location(error.span(), identifier_span(duplicate, "source", 1));

    let missing_directories_value = "app Example {
        name: \"example\",
        protocol: Protocol,
        managers: { directories: {} },
    }";
    let error = parse_error_with_span(missing_directories_value);

    assert!(error.to_string().contains("needs `app` or `root`"));
    assert_same_location(
        error.span(),
        group_span(missing_directories_value, Delimiter::Brace, 2),
    );

    let missing_config_source = "app Example {
        name: \"example\",
        protocol: Protocol,
        managers: { config: {} },
    }";
    let error = parse_error_with_span(missing_config_source);

    assert!(
        error
            .to_string()
            .contains("requires a `directories` manager")
    );
    assert_same_location(
        error.span(),
        identifier_span(missing_config_source, "config", 0),
    );

    let conflicting_directories = "app Example {
        name: \"example\",
        protocol: Protocol,
        managers: { directories: { app: \"example\", root: root() } },
    }";
    let error = parse_error_with_span(conflicting_directories);

    assert!(
        error
            .to_string()
            .contains("cannot combine `app` with `root`")
    );
    assert_same_location(
        error.span(),
        identifier_span(conflicting_directories, "root", 0),
    );

    let conflicting_config = "app Example {
        name: \"example\",
        protocol: Protocol,
        managers: {
            config: { source: source(), profiles: profiles() },
            directories: directories(),
        },
    }";
    let error = parse_error_with_span(conflicting_config);

    assert!(
        error
            .to_string()
            .contains("cannot combine `source` with `profiles`")
    );
    assert_same_location(
        error.span(),
        identifier_span(conflicting_config, "profiles", 0),
    );
}

#[test]
fn generated_command_collisions_use_the_user_command_span() {
    let help_collision = "app HelpCollision {
        name: \"help-collision\",
        protocol: Protocol,
        commands: { help: HelpCommand },
    }";
    let error = parse_error_with_span(help_collision);

    assert!(error.to_string().contains("framework reserved slot `help`"));
    assert_same_location(error.span(), identifier_span(help_collision, "help", 0));

    let serve_collision = "app ServeCollision {
        name: \"serve-collision\",
        protocol: Protocol,
        commands: { serve: ServeCommand },
        serve = lifecycle::serve,
    }";
    let error = parse_error_with_span(serve_collision);

    assert!(
        error
            .to_string()
            .contains("framework reserved slot `serve`")
    );
    assert_same_location(error.span(), identifier_span(serve_collision, "serve", 0));
}

#[test]
fn generated_app_host_items_follow_trait_definition_order() {
    let input = parse2::<NamedApp>(quote! {
        app OrderedApplication {
            name: "ordered-application",
            protocol: Protocol,
            setup = lifecycle::setup,
            configure = lifecycle::configure,
            before_build = lifecycle::before_build,
            after_build = lifecycle::after_build,
            serve = lifecycle::serve,
        }
    })
    .expect("ordered app parses");
    let output = expand(input);
    let file = parse2::<syn::File>(output).expect("expansion parses as a Rust file");
    let host = app_host_impl(&file);
    let items = host
        .items
        .iter()
        .map(|item| match item {
            syn::ImplItem::Type(item) => format!("type {}", item.ident),
            syn::ImplItem::Const(item) => format!("const {}", item.ident),
            syn::ImplItem::Fn(item) => format!("fn {}", item.sig.ident),
            _ => panic!("AppHost impl contains an unexpected item"),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        items,
        [
            "type Protocol",
            "const BOOTSTRAP_OWNS_CONFIG",
            "const BOOTSTRAP_OWNS_DIRECTORIES",
            "const LIFECYCLE_CAPABILITIES",
            "fn builder",
            "fn declare_plugins",
            "fn setup",
            "fn configure",
            "fn before_build",
            "fn after_build",
            "fn serve",
        ]
    );
}

#[test]
fn app_host_principal_declarations_preserve_key_semantics() {
    let source = "pub app SemanticApplication {
        name: \"semantic-application\",
        protocol: custom::SemanticProtocol,
        setup = lifecycle::semantic_setup,
        configure = lifecycle::semantic_configure,
        before_build = lifecycle::semantic_before_build,
        after_build = lifecycle::semantic_after_build,
        serve = lifecycle::semantic_serve,
    }";
    let input = TokenStream::from_str(source).expect("test token stream parses");
    let output = expand(parse2::<NamedApp>(input).expect("semantic app parses"));
    let file = parse2::<syn::File>(output.clone()).expect("expansion parses as a Rust file");
    let host = app_host_impl(&file);
    let protocol = host
        .items
        .iter()
        .find_map(|item| match item {
            syn::ImplItem::Type(item) if item.ident == "Protocol" => Some(item),
            _ => None,
        })
        .expect("AppHost impl contains Protocol associated type");

    assert_semantic_ident(
        &protocol.ident,
        "Protocol",
        identifier_span(source, "protocol", 0),
        "protocol",
    );

    let protocol_key = identifier_span(source, "protocol", 0);
    let mut protocol_spans = Vec::new();

    collect_identifier_spans(output.clone(), "Protocol", &mut protocol_spans);
    assert_eq!(
        protocol_spans
            .iter()
            .filter(|span| span.byte_range() == protocol_key.byte_range())
            .count(),
        1,
        "`protocol` key must anchor only the associated type declaration"
    );

    for method in ["setup", "configure", "before_build", "after_build", "serve"] {
        assert_semantic_ident(
            &impl_method(host, method).sig.ident,
            method,
            identifier_span(source, method, 0),
            method,
        );
    }

    for callback in [
        "SemanticProtocol",
        "semantic_setup",
        "semantic_configure",
        "semantic_before_build",
        "semantic_after_build",
        "semantic_serve",
    ] {
        assert!(
            expanded_identifier_locations(&output, callback)
                .contains(&identifier_span(source, callback, 0).start()),
            "expanded user token `{callback}` lost its source location"
        );
    }

    for method in ["setup", "configure", "before_build", "after_build", "serve"] {
        let key = identifier_span(source, method, 0);
        let mut spans = Vec::new();

        collect_identifier_spans(output.clone(), method, &mut spans);
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.byte_range() == key.byte_range())
                .count(),
            1,
            "`{method}` key must anchor only its AppHost declaration"
        );
    }
}

#[cfg(feature = "cli")]
#[test]
fn cli_principal_declarations_preserve_keys_without_anchoring_references() {
    let source = "pub app SemanticCliApplication {
        name: \"semantic-cli-application\",
        protocol: custom::SemanticProtocol,
        cli: {
            config: true,
            profile: true,
            log: true,
            log_format: true,
            color: true,
            serve: true,
        },
        serve = lifecycle::semantic_serve,
    }";
    let input = TokenStream::from_str(source).expect("test token stream parses");
    let output = expand(parse2::<NamedApp>(input).expect("semantic CLI app parses"));
    let file = parse2::<syn::File>(output.clone()).expect("expansion parses as a Rust file");
    let bootstrap = named_struct(&file, "SemanticCliApplicationBootstrapArgs");

    for (field, key) in [
        ("config", "config"),
        ("profiles", "profile"),
        ("log", "log"),
        ("log_format", "log_format"),
        ("color", "color"),
    ] {
        let source_span = identifier_span(source, key, 0);
        let ident = named_field(bootstrap, field)
            .ident
            .as_ref()
            .expect("bootstrap field is named");
        let mut spans = Vec::new();

        assert_semantic_ident(ident, field, source_span, key);
        collect_identifier_spans(output.clone(), field, &mut spans);
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.byte_range() == source_span.byte_range())
                .count(),
            1,
            "`{key}` key must anchor only its public bootstrap field"
        );
    }

    let policy_serve = identifier_span(source, "serve", 0);
    let public_serve = named_variant(named_enum(&file, "SemanticCliApplicationCommand"), "Serve");
    let private_serve = named_variant(
        named_enum(&file, "__SemanticCliApplicationFrameworkCommand"),
        "Serve",
    );
    let mut serve_spans = Vec::new();

    assert_semantic_ident(&public_serve.ident, "Serve", policy_serve, "serve");
    assert_not_source_anchored(&private_serve.ident, policy_serve, "serve");
    collect_identifier_spans(output, "Serve", &mut serve_spans);
    assert_eq!(
        serve_spans
            .iter()
            .filter(|span| span.byte_range() == policy_serve.byte_range())
            .count(),
        1,
        "the public Serve definition must be the only policy-key anchor"
    );
    for reference in serve_spans
        .iter()
        .filter(|span| span.byte_range() != policy_serve.byte_range())
    {
        if let Some(source_text) = reference.source_text() {
            assert_ne!(source_text, "serve");
        }
    }
}

#[cfg(feature = "cli")]
#[test]
fn cli_omitted_fields_and_fallback_serve_use_their_defined_anchors() {
    let source = "pub app DefaultCliApplication {
        name: \"default-cli-application\",
        protocol: SemanticProtocol,
        serve = lifecycle::semantic_serve,
    }";
    let input = TokenStream::from_str(source).expect("test token stream parses");
    let output = expand(parse2::<NamedApp>(input).expect("default CLI app parses"));
    let file = parse2::<syn::File>(output.clone()).expect("expansion parses as a Rust file");
    let app = identifier_span(source, "DefaultCliApplication", 0);
    let bootstrap = named_struct(&file, "DefaultCliApplicationBootstrapArgs");

    for field in ["config", "profiles", "log", "log_format", "color"] {
        let ident = named_field(bootstrap, field)
            .ident
            .as_ref()
            .expect("bootstrap field is named");

        assert_eq!(ident, field);
        assert_eq!(ident.span().byte_range(), app.byte_range());
    }

    let lifecycle_serve = identifier_span(source, "serve", 0);
    let public_serve = named_variant(named_enum(&file, "DefaultCliApplicationCommand"), "Serve");
    let private_serve = named_variant(
        named_enum(&file, "__DefaultCliApplicationFrameworkCommand"),
        "Serve",
    );
    let mut serve_spans = Vec::new();

    assert_semantic_ident(&public_serve.ident, "Serve", lifecycle_serve, "serve");
    assert_not_source_anchored(&private_serve.ident, lifecycle_serve, "serve");
    collect_identifier_spans(output, "Serve", &mut serve_spans);
    assert_eq!(
        serve_spans
            .iter()
            .filter(|span| span.byte_range() == lifecycle_serve.byte_range())
            .count(),
        1,
        "the public Serve definition must be the only lifecycle-key fallback anchor"
    );
}

#[cfg(feature = "cli")]
#[test]
fn cli_expansion_preserves_types_defaults_and_generated_identifier_anchors() {
    let source = "app AnchoredApplication {
        name: \"anchored\",
        protocol: custom::AnchoredProtocol,
        cli: { log: { default_value_t: defaults::typed_log() } },
        args: { global: args::GlobalArgs },
        commands: {
            namespace: {
                inspect: commands::InspectCommand,
            },
        },
        serve = lifecycle::serve,
    }";
    let input = TokenStream::from_str(source).expect("test token stream parses");
    let output = expand(parse2::<NamedApp>(input).expect("anchored app parses"));

    for identifier in [
        "AnchoredProtocol",
        "typed_log",
        "GlobalArgs",
        "InspectCommand",
    ] {
        let source_location = identifier_span(source, identifier, 0).start();

        assert!(
            expanded_identifier_locations(&output, identifier).contains(&source_location),
            "expanded `{identifier}` lost its source location"
        );
    }

    let app_location = identifier_span(source, "AnchoredApplication", 0).start();

    for generated in [
        "AnchoredApplicationCli",
        "AnchoredApplicationCommand",
        "AnchoredApplicationBootstrapArgs",
        "__AnchoredApplicationFrameworkCli",
        "__AnchoredApplicationFrameworkCommand",
    ] {
        assert!(
            expanded_identifier_locations(&output, generated).contains(&app_location),
            "generated `{generated}` is not anchored to the app identifier"
        );
    }

    let namespace_location = identifier_span(source, "namespace", 0).start();

    assert!(
        expanded_identifier_locations(&output, "AnchoredApplicationNamespaceCommand")
            .contains(&namespace_location)
    );
    assert!(
        expanded_identifier_locations(&output, "Inspect")
            .contains(&identifier_span(source, "inspect", 0).start())
    );
}

#[test]
fn parses_typed_framework_cli_customization() {
    parse2::<NamedApp>(quote! {
        app PolicyApplication {
            name: "policy-application",
            protocol: Protocol,
            cli: {
                config: {
                    name: "settings",
                    short: 's',
                    aliases: ["config"],
                    visible_aliases: ["configuration"],
                    hidden: false,
                    help: "Configuration source.",
                    value_name: "FILE",
                    default_value_t: ::std::path::PathBuf::from("config/application.toml"),
                },
                profile: {
                    default_values_t: [
                        ::std::string::String::from("base"),
                        ::std::string::String::from("local"),
                    ],
                },
                log: { default_value: "info" },
                log_format: { default_value_t: LogFormat::Json },
                color: { default_value: "never" },
                serve: {
                    name: "start",
                    aliases: ["run"],
                    visible_aliases: ["server"],
                    hidden: false,
                    help: "Start the service.",
                    default_command: false,
                },
            },
            serve = serve,
        }
    })
    .expect("reserved CLI policy parses");
}

#[test]
fn rejects_invalid_framework_cli_customization() {
    let slot = format_ident!("color");
    let value = "sometimes";
    let error = parse_error(quote! {
        app InvalidDefault {
            name: "invalid-default",
            protocol: Protocol,
            cli: { #slot: { default_value: #value } },
        }
    });

    assert!(error.contains(&format!("unsupported application default `{value}`")));
    assert!(
        parse_error(quote! {
            app DisabledDefaultServe {
                name: "disabled-default-serve",
                protocol: Protocol,
                cli: { serve: { enabled: false, default_command: true } },
                serve = serve,
            }
        })
        .contains("disabled `serve` cannot be the default command")
    );
    assert!(
        parse_error(quote! {
            app DisabledDefaultArgument {
                name: "disabled-default-argument",
                protocol: Protocol,
                cli: { log: { enabled: false, default_value: "info" } },
            }
        })
        .contains("disabled reserved CLI arguments cannot declare a default")
    );
    assert!(
        parse_error(quote! {
            app CustomServeCollision {
                name: "custom-serve-collision",
                protocol: Protocol,
                cli: { serve: { name: "start" } },
                commands: { start: StartCommand },
                serve = serve,
            }
        })
        .contains(
            "command name or alias `start` is claimed by framework reserved slot `serve` and application command `start`"
        )
    );
    assert!(
        parse_error(quote! {
            app HelpServe {
                name: "help-serve",
                protocol: Protocol,
                cli: { serve: { name: "help" } },
                serve = serve,
            }
        })
        .contains(
            "command name or alias `help` is claimed by framework reserved slot `help` and framework reserved slot `serve`"
        )
    );
}

#[test]
fn rejects_old_duplicate_mismatched_and_disabled_default_forms() {
    let removed_default = format_ident!("default");

    assert!(
        parse_error(quote! {
            app RemovedDefaultSyntax {
                name: "removed-default-syntax",
                protocol: Protocol,
                cli: { log: { #removed_default: "info" } },
            }
        })
        .contains("unknown reserved argument setting `default`")
    );
    assert!(
        parse_error(quote! {
            app CombinedDefaultForms {
                name: "combined-default-forms",
                protocol: Protocol,
                cli: { log: { default_value: "info", default_value_t: String::new() } },
            }
        })
        .contains("cannot combine `default_value` with `default_value_t`")
    );
    assert!(
        parse_error(quote! {
            app DuplicateDefaultForm {
                name: "duplicate-default-form",
                protocol: Protocol,
                cli: { log: { default_value: "info", default_value: "debug" } },
            }
        })
        .contains("duplicate reserved CLI setting `default_value`")
    );

    for (input, setting) in [
        (
            quote! {
                app SingularProfileDefault {
                    name: "singular-profile-default",
                    protocol: Protocol,
                    cli: { profile: { default_value: "local" } },
                }
            },
            "default_value",
        ),
        (
            quote! {
                app TypedSingularProfileDefault {
                    name: "typed-singular-profile-default",
                    protocol: Protocol,
                    cli: { profile: { default_value_t: String::from("local") } },
                }
            },
            "default_value_t",
        ),
        (
            quote! {
                app PluralLogDefault {
                    name: "plural-log-default",
                    protocol: Protocol,
                    cli: { log: { default_values: ["info"] } },
                }
            },
            "default_values",
        ),
        (
            quote! {
                app TypedPluralLogDefault {
                    name: "typed-plural-log-default",
                    protocol: Protocol,
                    cli: { log: { default_values_t: [String::from("info")] } },
                }
            },
            "default_values_t",
        ),
    ] {
        let error = parse_error(input);

        assert!(error.contains(&format!("does not accept `{setting}`")));
    }

    for setting in [
        quote!(default_value: "info"),
        quote!(default_values: ["local"]),
        quote!(default_value_t: String::from("info")),
        quote!(default_values_t: [String::from("local")]),
    ] {
        let slot = if setting.to_string().starts_with("default_values") {
            quote!(profile)
        } else {
            quote!(log)
        };
        let error = parse_error(quote! {
            app DisabledDefaultForm {
                name: "disabled-default-form",
                protocol: Protocol,
                cli: { #slot: { enabled: false, #setting } },
            }
        });

        assert!(error.contains("disabled reserved CLI arguments cannot declare a default"));
    }
}

#[cfg(feature = "cli")]
#[test]
fn expands_app_specific_bootstrap_and_framework_parser() {
    let input = parse2::<NamedApp>(quote! {
        app PolicyApplication {
            name: "policy-application",
            protocol: Protocol,
            cli: {
                config: {
                    name: "settings",
                    short: false,
                    default_value_t: ::std::path::PathBuf::from("config.toml"),
                },
                profile: {
                    default_values_t: [
                        ::std::string::String::from("base"),
                        ::std::string::String::from("local"),
                    ],
                },
                log: { default_value: "info" },
                log_format: { default_value_t: LogFormat::Json },
                color: { default_value: "never" },
                serve: { name: "start", default_command: false },
            },
            serve = serve,
        }
    })
    .expect("policy application parses");
    let output = expand(input).to_string();

    assert!(output.contains("struct PolicyApplicationBootstrapArgs"));
    assert!(output.contains("long = \"settings\""));
    assert!(output.contains("long = \"profile\""));
    assert!(
        output.contains("default_value_t = :: std :: path :: PathBuf :: from (\"config.toml\")")
    );
    assert!(output.contains("pub config : :: std :: path :: PathBuf"));
    assert!(output.contains("default_values_t = ["));
    assert!(output.contains(":: std :: string :: String :: from (\"base\")"));
    assert!(output.contains(":: std :: string :: String :: from (\"local\")"));
    assert!(output.contains("pub profiles : Vec < :: std :: string :: String >"));
    assert!(output.contains("default_value = \"info\""));
    assert!(output.contains("pub log : Option < :: std :: string :: String >"));
    assert!(output.contains("default_value_t = LogFormat :: Json"));
    assert!(output.contains("pub log_format : :: upwell :: LogFormat"));
    assert!(output.contains("default_value = \"never\""));
    assert!(output.contains("pub color : Option < :: upwell :: ColorChoice >"));
    assert!(output.contains("name = \"start\""));
    assert!(output.contains("struct __PolicyApplicationFrameworkCli"));
    assert!(output.contains("enum __PolicyApplicationFrameworkCommand"));
    assert!(output.contains("command : Option < __PolicyApplicationFrameworkCommand >"));
    assert!(output.contains("ValueSource"));
    assert!(output.contains("Self :: Serve"));
    assert!(!output.contains("__UpwellServe"));

    let input = parse2::<NamedApp>(quote! {
        app AdditionalDefaultForms {
            name: "additional-default-forms",
            protocol: Protocol,
            cli: {
                profile: { default_values: ["base", "local"] },
                log: { default_value_t: ::std::string::String::from("info") },
                color: { default_value_t: ColorChoice::Never },
            },
            commands: { inspect: InspectCommand },
        }
    })
    .expect("additional default forms parse");
    let output = expand(input).to_string();

    assert!(output.contains("default_values = [\"base\" , \"local\"]"));
    assert!(output.contains("default_value_t = :: std :: string :: String :: from (\"info\")"));
    assert!(output.contains("pub log : :: std :: string :: String"));
    assert!(output.contains("default_value_t = ColorChoice :: Never"));
    assert!(output.contains("pub color : :: upwell :: ColorChoice"));
}

#[cfg(feature = "cli")]
#[test]
fn rejects_enabled_serve_without_a_serve_phase() {
    let source = "app MissingServePhase {
        name: \"missing-serve-phase\",
        protocol: Protocol,
        cli: { serve: true },
    }";
    let input =
        parse2::<NamedApp>(TokenStream::from_str(source).expect("test token stream parses"))
            .expect("policy syntax parses");
    let output = expand(input);
    let serve = identifier_span(source, "serve", 0);
    let compile_error = output
        .into_iter()
        .find_map(|token| match token {
            TokenTree::Ident(ident) if ident == "compile_error" => Some(ident.span()),
            _ => None,
        })
        .expect("expansion contains compile_error");

    assert_same_location(compile_error, serve);
}

#[cfg(feature = "cli")]
#[test]
fn cli_literal_name_error_uses_name_expression_span() {
    let source = "app DynamicName {
        name: dynamic::application_name(),
        protocol: Protocol,
    }";
    let input =
        parse2::<NamedApp>(TokenStream::from_str(source).expect("test token stream parses"))
            .expect("dynamic-name syntax parses");
    let output = expand(input);
    let expected = identifier_span(source, "dynamic", 0).start();
    let compile_error_locations = expanded_identifier_locations(&output, "compile_error");

    assert!(compile_error_locations.contains(&expected));
    assert!(
        output
            .to_string()
            .contains("require a string literal `name`")
    );
}

#[cfg(feature = "tooling")]
#[test]
fn tooling_literal_name_error_uses_name_expression_span() {
    let source = "app DynamicToolingName {
        name: dynamic::application_name(),
        protocol: Protocol,
    }";
    let input =
        parse2::<NamedApp>(TokenStream::from_str(source).expect("test token stream parses"))
            .expect("dynamic-name syntax parses");
    let output = expand(input);
    let expected = identifier_span(source, "dynamic", 0).start();
    let compile_error_locations = expanded_identifier_locations(&output, "compile_error");

    assert!(compile_error_locations.contains(&expected));
    assert!(
        output
            .to_string()
            .contains("require a string literal `name`")
    );
}

#[test]
fn invalid_serve_policy_combination_uses_key_span() {
    let source = "app InvalidServePolicy {
        name: \"invalid-serve-policy\",
        protocol: Protocol,
        cli: { serve: { enabled: false, default_command: true } },
        serve = lifecycle::run,
    }";
    let error = parse_error_with_span(source);

    assert!(error.to_string().contains("cannot be the default command"));
    assert_same_location(error.span(), identifier_span(source, "serve", 0));
}

#[cfg(feature = "cli")]
#[test]
fn enabled_serve_error_remains_source_local_compile_error() {
    let input = parse2::<NamedApp>(quote! {
        app MissingServePhase {
            name: "missing-serve-phase",
            protocol: Protocol,
            cli: { serve: true },
        }
    })
    .expect("policy syntax parses");
    let output = expand(input).to_string();

    assert!(output.contains("cannot be enabled without a declared serve phase"));
}

#[test]
fn expands_static_plugin_declarations_on_the_host() {
    let input = parse2::<NamedApp>(quote! {
        app Example {
            name: "example",
            protocol: Protocol,
            plugins: [Plugin, replace SLOT => Replacement, suppress OPTIONAL_SLOT],
        }
    })
    .expect("static plugins parse");
    let output = expand(input).to_string();

    assert!(output.contains("fn declare_plugins"));
    assert!(output.contains("plugins . register :: < Plugin >"));
    assert!(output.contains("plugins . replace_with :: < Replacement > (SLOT)"));
    assert!(output.contains("plugins . suppress (OPTIONAL_SLOT)"));
}

#[test]
fn rejects_duplicate_named_app_keys() {
    for (input, key) in [
        (
            quote!(app Example { name: "one", name: "two", protocol: Protocol }),
            "name",
        ),
        (
            quote!(app Example { name: "one", protocol: First, protocol: Second }),
            "protocol",
        ),
        (
            quote!(app Example { name: "one", protocol: Protocol, components: [], components: [] }),
            "components",
        ),
    ] {
        let error = parse_error(input);

        assert_eq!(error, format!("duplicate app key `{key}`"));
    }
}

#[test]
fn rejects_incomplete_and_unknown_named_apps() {
    assert!(parse_error(quote!(app Example { protocol: Protocol })).contains("requires a `name`"));
    assert!(parse_error(quote!(app Example { name: "example" })).contains("requires a `protocol"));
    assert!(
        parse_error(quote!(app Example {
            name: "example",
            protocol: Protocol,
            unknown: true,
        }))
        .contains("unknown `app!` key `unknown`")
    );
}

#[test]
fn expands_named_host_and_builder() {
    let input = parse2::<NamedApp>(quote! {
        pub app Example {
            name: "example",
            protocol: Protocol,
            components: [component()],
        }
    })
    .expect("named app parses");
    let output = expand(input).to_string();

    assert!(output.contains("pub struct Example"));
    assert!(output.contains("Stage : :: upwell :: AppStage < Protocol > = :: upwell :: Initial"));
    assert!(output.contains("impl Example < :: upwell :: Initial >"));
    assert!(output.contains("impl Example < :: upwell :: Setup >"));
    assert!(output.contains("impl Example < :: upwell :: PreBuild >"));
    assert!(output.contains("impl Example < :: upwell :: Built >"));
    assert!(!output.contains("__upwell_setup"));
    assert!(!output.contains("__upwell_prepare"));
    assert!(!output.contains("__upwell_build"));
    assert!(output.contains(
        "pub fn builder () -> :: core :: result :: Result < :: upwell :: AppBuilder < Protocol > , :: upwell :: ConfigError >"
    ));
    assert!(output.contains("Result :: Ok"));
    assert_eq!(output.matches("with_component").count(), 1);
}

#[test]
fn named_builder_propagates_directory_config_errors() {
    let input = parse2::<NamedApp>(quote! {
        pub app Example {
            name: "example",
            protocol: Protocol,
            managers: {
                directories: { root: root() },
                config: {},
            },
        }
    })
    .expect("named app parses");
    let output = expand(input).to_string();

    assert!(output.contains("ConfigManager :: < :: upwell :: config :: Dynamic > :: load_from"));
    assert!(output.contains("?"));
    assert!(output.contains("Result < :: upwell :: AppBuilder"));
}

#[test]
fn rejects_expression_form() {
    assert!(parse_error(quote!(name: "legacy", protocol: Protocol)).contains("expected `app`"));
}

#[test]
fn parses_external_and_inline_lifecycle_phases() {
    let input = quote! {
        pub app Example {
            name: "example",
            protocol: Protocol,
            setup = setup,
            configure(context, builder) { Ok(builder) },
            before_build = before_build,
            after_build(context, app) { Ok(app) },
            serve = serve,
        }
    };

    parse2::<NamedApp>(input).expect("lifecycle phases parse");
}

#[test]
fn rejects_invalid_lifecycle_phase_forms() {
    assert!(
        parse_error(quote! {
            app Example {
                name: "example",
                protocol: Protocol,
                setup: {},
            }
        })
        .contains("declarative lifecycle settings are reserved")
    );
    assert!(
        parse_error(quote! {
            app Example {
                name: "example",
                protocol: Protocol,
                setup(context, extra) { Ok(context) },
            }
        })
        .contains("`setup` expects 1 argument")
    );
}

#[cfg(feature = "cli")]
#[test]
fn generated_cli_requires_literal_application_name() {
    let input = parse2::<NamedApp>(quote! {
        app DynamicName {
            name: String::from("dynamic"),
            protocol: Protocol,
            serve = serve,
        }
    })
    .expect("app syntax parses");
    let output = expand(input).to_string();

    assert!(output.contains("require a string literal `name`"));
}

#[test]
fn generated_cli_preserves_explicit_manager_policy() {
    let input = parse2::<NamedApp>(quote! {
        app ExplicitManagers {
            name: "explicit-managers",
            protocol: Protocol,
            managers: {
                directories: directories,
                config: config,
            },
            serve = serve,
        }
    })
    .expect("app syntax parses");
    let output = expand(input).to_string();

    assert!(!output.contains("configure_bootstrap_directories"));
    assert!(!output.contains("configure_bootstrap_config"));
}

#[cfg(not(feature = "cli"))]
#[test]
fn named_app_without_cli_omits_bootstrap_helpers() {
    let input = parse2::<NamedApp>(quote! {
        app NoCli {
            name: "no-cli",
            protocol: Protocol,
        }
    })
    .expect("app syntax parses");
    let output = expand(input).to_string();

    assert!(!output.contains("finalize_bootstrap"));
    assert!(!output.contains("configure_bootstrap"));
}

#[cfg(feature = "cli")]
#[test]
fn named_app_without_application_commands_generates_plugin_cli_shell() {
    let input = parse2::<NamedApp>(quote! {
        app PluginOnly {
            name: "plugin-only",
            protocol: Protocol,
            plugins: [Plugin],
        }
    })
    .expect("plugin-only app parses");
    let output = expand(input).to_string();

    assert!(output.contains("struct PluginOnlyCli"));
    assert!(output.contains("fn run_with"));
    assert!(!output.contains("enum PluginOnlyCommand"));
    assert!(!output.contains("fn run_cli"));
}

#[cfg(feature = "cli")]
#[test]
fn generated_cli_exposes_only_source_preserving_runners() {
    let input = parse2::<NamedApp>(quote! {
        app Commands {
            name: "commands",
            protocol: Protocol,
            commands: { inspect: Inspect },
        }
    })
    .expect("command app parses");
    let output = expand(input).to_string();

    assert!(output.contains("fn run_with"));
    assert!(!output.contains("fn run_cli"));
    assert!(!output.contains("__into_explicit_options"));
}

#[cfg(feature = "tooling")]
#[test]
fn named_app_generates_target_local_tooling_entry() {
    let input = parse2::<NamedApp>(quote! {
        app ToolingApplication {
            name: "tooling-application",
            protocol: Protocol,
        }
    })
    .expect("tooling application parses");
    let output = expand(input).to_string();

    assert!(output.contains("pub async fn tooling_probe"));
    assert!(output.contains("ProbeTargetIdentity"));
    assert!(!output.contains("CARGO_CRATE_NAME"));
    assert!(output.contains("file ! ()"));
    assert!(output.contains("__private :: probe_"));
    assert!(output.contains("__private :: catch_probe_panic"));
    #[cfg(feature = "cli")]
    {
        assert!(output.contains("Self :: __upwell_parse_cli"));
        assert!(output.contains("ExecutionMode :: Tooling"));
    }
}

#[cfg(feature = "tooling")]
#[test]
fn generated_tooling_identity_requires_literal_application_name() {
    let input = parse2::<NamedApp>(quote! {
        app ToolingApplication {
            name: application_name(),
            protocol: Protocol,
        }
    })
    .expect("tooling application parses");
    let output = expand(input).to_string();

    assert!(output.contains("require a string literal"));
}

#[cfg(all(feature = "cli", feature = "tooling"))]
#[test]
fn generated_run_recognizes_exact_hidden_probe_contract_before_clap() {
    let input = parse2::<NamedApp>(quote! {
        app ToolingApplication {
            name: "tooling-application",
            protocol: Protocol,
        }
    })
    .expect("tooling application parses");
    let output = expand(input).to_string();

    assert!(output.contains("TOOLING_PROBE_ARGUMENT"));
    assert!(output.contains("arguments . len () == 2"));
    assert!(output.contains("__private :: install_process_probe_panic_hook"));
    assert!(output.contains("__private :: probe_target_identity_from_env"));
    assert!(output.contains("Self :: __upwell_tooling_probe (target)"));
    assert!(output.contains("__private :: emit_probe_envelope_from_env"));
}

mod builder;
