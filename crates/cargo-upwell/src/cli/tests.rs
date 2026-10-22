use std::ffi::OsString;

use clap::{CommandFactory as _, Parser};

use super::format::{Graph, Inspect, format_candidates};
use super::{Cli, CommandRequest, InspectResourceKind, TerminalPolicy, normalized_arguments};
use cargo_upwell::{CommandKind, GraphDirection, GraphRelationFamily};

#[test]
fn completions_generate_parses_every_supported_shell() {
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let cli = Cli::try_parse_from(["cargo-upwell", "completions", "generate", shell])
            .expect("supported completion shell parses");
        let CommandRequest::GenerateCompletions(parsed) = cli.into_request() else {
            panic!("completion generation produces the expected request");
        };

        assert_eq!(parsed.to_string(), shell);
    }
}

#[test]
fn completions_refresh_reuses_target_selection() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "completions",
        "refresh",
        "--package",
        "example",
        "--bin",
        "server",
        "--features",
        "http,metrics",
    ])
    .expect("completion refresh arguments parse");
    let CommandRequest::RefreshCompletions(request) = cli.into_request() else {
        panic!("completion refresh produces a discovery request");
    };

    assert_eq!(request.package.as_deref(), Some("example"));
    assert_eq!(request.binary.as_deref(), Some("server"));
    assert_eq!(request.features.normalized_features(), ["http", "metrics"]);
}

#[test]
fn version_uses_the_rich_build_report() {
    let error =
        Cli::try_parse_from(["cargo-upwell", "--version"]).expect_err("version exits through Clap");
    let output = error.to_string();

    assert!(output.contains(env!("CARGO_PKG_VERSION")));
    assert!(output.contains("tooling schema"));
    assert!(output.contains("\ngit "));
    assert!(output.contains("\nbuilt for "));
}

#[test]
fn init_parses_catalog_and_template_values() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "init",
        "projects/service",
        "--name",
        "service-api",
        "--template",
        "team/application",
        "--catalog",
        "upwell-catalog.toml",
        "--no-vcs",
        "--define",
        "database=postgres",
        "--upwell-path",
        "../upwell",
    ])
    .expect("init arguments parse");
    let CommandRequest::Init(request) = cli.into_request() else {
        panic!("init produces an init request");
    };

    assert_eq!(
        request.destination,
        std::path::Path::new("projects/service")
    );
    assert_eq!(request.name.as_deref(), Some("service-api"));
    assert_eq!(
        request.template,
        cargo_upwell::TemplateSelection::Catalog {
            template: Some(String::from("team/application")),
            catalog_path: Some(std::path::PathBuf::from("upwell-catalog.toml")),
        }
    );
    assert!(request.no_vcs);
    assert_eq!(request.define, ["database=postgres"]);
    assert_eq!(
        request.upwell_path.as_deref(),
        Some(std::path::Path::new("../upwell"))
    );
}

#[test]
fn init_rejects_catalog_template_and_direct_path_together() {
    let error = Cli::try_parse_from([
        "cargo-upwell",
        "init",
        "service",
        "--template",
        "team/application",
        "--template-path",
        "templates/app",
    ])
    .expect_err("template selectors conflict");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn init_rejects_catalog_file_and_direct_path_together() {
    let error = Cli::try_parse_from([
        "cargo-upwell",
        "init",
        "service",
        "--catalog",
        "catalog.toml",
        "--template-path",
        "templates/app",
    ])
    .expect_err("catalog and direct path conflict");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn init_without_a_template_defers_to_interactive_selection() {
    let cli = Cli::try_parse_from(["cargo-upwell", "init", "service"])
        .expect("interactive init arguments parse");
    let CommandRequest::Init(request) = cli.into_request() else {
        panic!("init produces an init request");
    };

    assert_eq!(
        request.template,
        cargo_upwell::TemplateSelection::Catalog {
            template: None,
            catalog_path: None,
        }
    );
}

#[test]
fn templates_parses_an_explicit_catalog() {
    let cli = Cli::try_parse_from(["cargo-upwell", "templates", "--catalog", "catalog.toml"])
        .expect("template listing arguments parse");
    let CommandRequest::Templates { catalog_path } = cli.into_request() else {
        panic!("templates produces a catalog request");
    };

    assert_eq!(
        catalog_path.as_deref(),
        Some(std::path::Path::new("catalog.toml"))
    );
}

#[test]
fn cargo_external_subcommand_name_is_removed_before_parsing() {
    let arguments = normalized_arguments([
        OsString::from("cargo-upwell"),
        OsString::from("upwell"),
        OsString::from("check"),
        OsString::from("--format"),
        OsString::from("json"),
    ]);
    let cli = Cli::try_parse_from(arguments).expect("Cargo-style arguments parse");
    let CommandRequest::Report {
        command, format, ..
    } = cli.into_request()
    else {
        panic!("check produces a report request");
    };

    assert_eq!(command, CommandKind::Check);
    assert_eq!(format, "json");
}

#[test]
fn direct_binary_arguments_remain_supported() {
    let arguments = normalized_arguments([
        OsString::from("cargo-upwell"),
        OsString::from("doctor"),
        OsString::from("--package"),
        OsString::from("fixture"),
        OsString::from("--bin"),
        OsString::from("fixture-bin"),
        OsString::from("--features"),
        OsString::from("tooling,cli"),
    ]);
    let cli = Cli::try_parse_from(arguments).expect("direct arguments parse");
    let CommandRequest::Report {
        command,
        discovery: request,
        format,
    } = cli.into_request()
    else {
        panic!("doctor produces a report request");
    };

    assert_eq!(command, CommandKind::Doctor);
    assert_eq!(request.package.as_deref(), Some("fixture"));
    assert_eq!(request.binary.as_deref(), Some("fixture-bin"));
    assert_eq!(request.features.features, ["tooling", "cli"]);
    assert_eq!(format, "terminal");
}

#[test]
fn inspect_parses_filters_and_terminal_policy() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "inspect",
        "--format",
        "text",
        "--kind",
        "component,provider",
        "--resource",
        "Worker",
        "--color",
        "never",
        "--pager",
        "always",
    ])
    .expect("inspect arguments parse");
    let CommandRequest::Inspect {
        format,
        filters,
        color,
        pager,
        ..
    } = cli.into_request()
    else {
        panic!("inspect produces an inspect request");
    };

    assert_eq!(format, "text");
    assert_eq!(
        filters.kinds,
        [
            InspectResourceKind::Component,
            InspectResourceKind::Provider
        ]
    );
    assert_eq!(filters.resources, ["Worker"]);
    assert_eq!(color, TerminalPolicy::Never);
    assert_eq!(pager, TerminalPolicy::Always);
}

#[test]
fn export_parses_payload_and_output_path() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "export",
        "--format",
        "envelope",
        "--output",
        "inspection.json",
    ])
    .expect("export arguments parse");
    let CommandRequest::Export { format, output, .. } = cli.into_request() else {
        panic!("export produces an export request");
    };

    assert_eq!(format, "envelope");
    assert_eq!(
        output.as_deref(),
        Some(std::path::Path::new("inspection.json"))
    );
}

#[test]
fn graph_preserves_repeated_whole_value_selectors() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "graph",
        "--format",
        "mermaid",
        "--family",
        "composition",
        "--direction",
        "upstream",
        "--resource",
        "component:a,b",
        "--resource",
        "Comma, Name",
        "--contributor",
        "plugin:a,b",
        "--plugin",
        "Plugin, Name",
    ])
    .expect("graph arguments parse");
    let CommandRequest::Graph { format, query, .. } = cli.into_request() else {
        panic!("graph produces a graph request");
    };

    assert_eq!(format, "mermaid");
    assert_eq!(query.family, GraphRelationFamily::Composition);
    assert_eq!(query.direction, GraphDirection::Upstream);
    assert_eq!(query.resources, ["component:a,b", "Comma, Name"]);
    assert_eq!(query.contributors, ["plugin:a,b"]);
    assert_eq!(query.plugins, ["Plugin, Name"]);
}

#[test]
fn explain_preserves_one_complete_resource_value() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "explain",
        "Resource, With, Commas",
        "--format",
        "json",
        "--color",
        "never",
    ])
    .expect("explain arguments parse");
    let CommandRequest::Explain {
        format,
        resource,
        color,
        ..
    } = cli.into_request()
    else {
        panic!("explain produces an explanation request");
    };

    assert_eq!(format, "json");
    assert_eq!(resource, "Resource, With, Commas");
    assert_eq!(color, TerminalPolicy::Never);
}

#[test]
fn explain_collects_repeated_config_assignments_into_a_preview() {
    let cli = Cli::try_parse_from([
        "cargo-upwell",
        "explain",
        "dyn Authenticator",
        "--set",
        "auth.custom.enabled=false",
        "--set",
        "auth.realm=\"a=b\"",
    ])
    .expect("explain arguments parse");
    let CommandRequest::Explain { preview, .. } = cli.into_request() else {
        panic!("explain produces an explanation request");
    };

    let assignments = preview
        .expect("assignments request a preview")
        .assignments
        .into_iter()
        .map(|assignment| (assignment.path, assignment.value))
        .collect::<Vec<_>>();

    assert_eq!(
        assignments,
        [
            (String::from("auth.custom.enabled"), String::from("false")),
            (String::from("auth.realm"), String::from("\"a=b\"")),
        ]
    );
}

#[test]
fn explain_without_assignments_requests_no_preview() {
    let cli = Cli::try_parse_from(["cargo-upwell", "explain", "component:worker"])
        .expect("explain arguments parse");
    let CommandRequest::Explain { preview, .. } = cli.into_request() else {
        panic!("explain produces an explanation request");
    };

    assert_eq!(preview, None);
}

#[test]
fn explain_rejects_malformed_assignments() {
    for argument in [
        "auth.custom",
        "=true",
        "auth..custom=true",
        "auth custom=true",
    ] {
        assert!(
            Cli::try_parse_from(["cargo-upwell", "explain", "x", "--set", argument]).is_err(),
            "{argument:?} is rejected"
        );
    }
}

#[test]
fn cargo_help_uses_external_subcommand_invocation_name() {
    let mut command = Cli::command();
    let mut output = Vec::new();

    command
        .write_long_help(&mut output)
        .expect("long help writes");

    let output = String::from_utf8(output).expect("help is UTF-8");

    assert!(output.contains("Usage: cargo upwell <COMMAND>"));
}

#[test]
fn graph_help_lists_registry_formats() {
    let mut command = Cli::command();
    let graph = command
        .find_subcommand_mut("graph")
        .expect("graph command exists");
    let mut output = Vec::new();

    graph.write_long_help(&mut output).expect("help writes");
    let output = String::from_utf8(output).expect("help is UTF-8");

    assert!(output.contains("[possible values: dot, json, mermaid, text]"));
}

#[test]
fn cargo_help_uses_colored_cargo_style_headings_and_literals() {
    let styles = Cli::command().get_styles().clone();

    assert_eq!(styles.get_header().to_string(), "\u{1b}[1m\u{1b}[92m");
    assert_eq!(styles.get_literal().to_string(), "\u{1b}[1m\u{1b}[96m");
}

#[test]
fn format_completion_is_scoped_by_the_generic_command_type() {
    let inspect = format_candidates::<Inspect>(std::ffi::OsStr::new(""))
        .into_iter()
        .map(|candidate| candidate.get_value().to_owned())
        .collect::<Vec<_>>();
    let graph = format_candidates::<Graph>(std::ffi::OsStr::new(""))
        .into_iter()
        .map(|candidate| candidate.get_value().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(inspect, ["json", "text"]);
    assert_eq!(graph, ["dot", "json", "mermaid", "text"]);
}
