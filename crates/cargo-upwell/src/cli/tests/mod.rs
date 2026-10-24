use std::ffi::OsString;

use clap::{CommandFactory as _, Parser};

use super::format::{Graph, Inspect, format_candidates};
use super::{Cli, CommandRequest, InspectResourceKind, TerminalPolicy, normalized_arguments};
use cargo_upwell::{CommandKind, GraphDirection, GraphRelationFamily};

mod init;
mod inspection;

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
