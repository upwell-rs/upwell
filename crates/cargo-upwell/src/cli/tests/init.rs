use super::*;

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
