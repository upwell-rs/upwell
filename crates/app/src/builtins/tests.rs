use upwell_config::{ConfigManager, ConfigProperties, ResolverChain, Toml};

use super::config::{LogFormat, LoggingConfig, ServerConfig, SpanEvents};

#[test]
fn server_config_round_trips_a_subtree() {
    const TOML: &str = r#"
        [server]
        bind = "${BUILTINS_TEST_BIND:0.0.0.0}"
        port = 8080
    "#;

    let tree = ConfigManager::<Toml>::from_str(TOML)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty());
    let value: ServerConfig = tree.get("server").expect("bind server config");

    assert_eq!(
        value,
        ServerConfig {
            bind: "0.0.0.0".to_string(),
            port: 8080,
        }
    );
}

#[test]
fn logging_config_round_trips_a_subtree() {
    const TOML: &str = r#"
        [logging]
        level = "${BUILTINS_TEST_LEVEL:info}"
        format = "compact"
        ansi = false

        span_events = "active"
        target = false
        level_display = false
        thread_ids = true
        thread_names = true
        file = true
        line_number = true
        flatten_event = true
        current_span = false
    "#;

    let tree = ConfigManager::<Toml>::from_str(TOML)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty());
    let value: LoggingConfig = tree.get("logging").expect("bind logging config");

    assert_eq!(
        value,
        LoggingConfig {
            level: "info".to_string(),
            format: LogFormat::Compact,
            ansi: false,
            span_events: SpanEvents::Active,
            target: false,
            level_display: false,
            thread_ids: true,
            thread_names: true,
            file: true,
            line_number: true,
            flatten_event: true,
            current_span: false,
        }
    );
}

#[test]
fn unknown_log_format_is_rejected_during_extraction() {
    let tree = ConfigManager::<Toml>::from_str(
        r#"
            [logging]
            level = "info"
            format = "xml"
            ansi = false
        "#,
    )
    .expect("parse config");
    let result = tree.get::<LoggingConfig>("logging");

    assert!(result.is_err());
}

#[test]
fn log_format_catalog_drives_typed_parsing_and_display() {
    let names = LogFormat::names().collect::<Vec<_>>();
    let formats = names
        .iter()
        .map(|name| name.parse::<LogFormat>().expect("catalog entry parses"))
        .collect::<Vec<_>>();

    assert_eq!(
        formats,
        [
            LogFormat::Full,
            LogFormat::Compact,
            LogFormat::Pretty,
            LogFormat::Json,
        ]
    );
    assert_eq!(
        formats.iter().map(ToString::to_string).collect::<Vec<_>>(),
        names
    );
    let error = "yaml"
        .parse::<LogFormat>()
        .expect_err("unknown format fails")
        .to_string();

    assert!(error.starts_with("expected one of: "));
    assert!(names.iter().all(|name| error.contains(name)));
}

#[test]
fn config_property_names_are_stable() {
    assert_eq!(<ServerConfig as ConfigProperties>::NAME, "ServerConfig");
    assert_eq!(<LoggingConfig as ConfigProperties>::NAME, "LoggingConfig");
}
