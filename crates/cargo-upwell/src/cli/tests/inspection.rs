use super::*;

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
