use std::net::{IpAddr, SocketAddr};

use upwell_config::{ConfigManager, ResolverChain, Toml};

use super::{AXUM_CONFIG_PATH, AxumConfig, validate_mount_path};

#[test]
fn defaults_materialize_without_an_axum_subtree() {
    let manager = ConfigManager::<Toml>::empty()
        .with_resolvers(ResolverChain::empty())
        .with_config::<AxumConfig>(AXUM_CONFIG_PATH);
    let config = manager
        .get_config::<AxumConfig>(AXUM_CONFIG_PATH)
        .expect("default axum config");

    assert_eq!(config, AxumConfig::default());
    assert_eq!(
        config.socket_addr(),
        SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 3000)
    );
}

#[test]
fn configured_listener_overrides_the_defaults() {
    let manager = ConfigManager::<Toml>::from_str(
        r#"
            [axum]
            bind = "0.0.0.0"
            port = 8080
            max_request_body_bytes = 1048576
            max_stream_request_bytes = 8388608
            max_stream_request_items = 50000
            stream_request_timeout_ms = 7000
            max_websocket_message_bytes = 131072
            max_websocket_frame_bytes = 32768
            max_websocket_connections = 128
            websocket_idle_timeout_ms = 20000
            request_timeout_ms = 15000
            graceful_shutdown_timeout_ms = 5000
        "#,
    )
    .expect("parse config")
    .with_resolvers(ResolverChain::empty())
    .with_config::<AxumConfig>(AXUM_CONFIG_PATH);

    let config = manager
        .get_config::<AxumConfig>(AXUM_CONFIG_PATH)
        .expect("configured axum listener");

    assert_eq!(
        config.socket_addr(),
        SocketAddr::new(IpAddr::from([0, 0, 0, 0]), 8080)
    );
    assert_eq!(config.max_request_body_bytes, 1_048_576);
    assert_eq!(config.max_stream_request_bytes, 8_388_608);
    assert_eq!(config.max_stream_request_items, 50_000);
    assert_eq!(config.stream_request_timeout_ms, 7_000);
    assert_eq!(config.max_websocket_message_bytes, 131_072);
    assert_eq!(config.max_websocket_frame_bytes, 32_768);
    assert_eq!(config.max_websocket_connections, 128);
    assert_eq!(config.websocket_idle_timeout_ms, 20_000);
    assert_eq!(config.request_timeout_ms, 15_000);
    assert_eq!(config.graceful_shutdown_timeout_ms, 5_000);
}

#[test]
fn base_path_defaults_empty_and_is_configurable() {
    assert_eq!(AxumConfig::default().base_path, "");

    let manager = ConfigManager::<Toml>::from_str(
        r#"
            [axum]
            base_path = "/api"
        "#,
    )
    .expect("parse config")
    .with_resolvers(ResolverChain::empty())
    .with_config::<AxumConfig>(AXUM_CONFIG_PATH);
    let config = manager
        .get_config::<AxumConfig>(AXUM_CONFIG_PATH)
        .expect("configured axum base path");

    assert_eq!(config.base_path, "/api");
}

#[cfg(feature = "openapi")]
#[test]
fn openapi_config_defaults_disabled_json_only() {
    use super::{AXUM_OPENAPI_CONFIG_PATH, OpenApiConfig, OpenApiUi};

    let manager = ConfigManager::<Toml>::empty()
        .with_resolvers(ResolverChain::empty())
        .with_config::<OpenApiConfig>(AXUM_OPENAPI_CONFIG_PATH);
    let config = manager
        .get_config::<OpenApiConfig>(AXUM_OPENAPI_CONFIG_PATH)
        .expect("default openapi config");

    assert!(!config.enabled, "OpenAPI is off unless explicitly enabled");
    assert_eq!(config.json_path, "/openapi.json");
    assert_eq!(config.ui, OpenApiUi::None);
    assert_eq!(config, OpenApiConfig::default());
}

#[cfg(feature = "openapi")]
#[test]
fn openapi_config_selects_ui() {
    use super::{AXUM_OPENAPI_CONFIG_PATH, OpenApiConfig, OpenApiUi};

    let manager = ConfigManager::<Toml>::from_str(
        r#"
            [axum.openapi]
            enabled = true
            ui = "swagger"
        "#,
    )
    .expect("parse config")
    .with_resolvers(ResolverChain::empty())
    .with_config::<OpenApiConfig>(AXUM_OPENAPI_CONFIG_PATH);
    let config = manager
        .get_config::<OpenApiConfig>(AXUM_OPENAPI_CONFIG_PATH)
        .expect("configured openapi");

    assert!(config.enabled);
    assert_eq!(config.ui, OpenApiUi::Swagger);
}

#[test]
fn static_absolute_mount_paths_are_required() {
    assert!(validate_mount_path("test path", "/api/v1").is_ok());
    assert!(validate_mount_path("test path", "api").is_err());
    assert!(validate_mount_path("test path", "/{*tail}").is_err());
    assert!(validate_mount_path("test path", "/api//v1").is_err());
}

#[test]
fn protocol_configs_declare_their_documented_fields_for_tooling() {
    use upwell_config::ConfigProperties;
    use upwell_config::schema::ConfigShape;

    let ConfigShape::Struct(fields) = AxumConfig::SCHEMA.shape else {
        panic!("AxumConfig declares a struct schema");
    };
    let port = fields
        .iter()
        .find(|field| field.name == "port")
        .expect("the port field is declared");

    assert_eq!(port.declared_type, "u16");
    assert_eq!(port.default, Some("${AXUM_PORT:3000}"));
    assert!(port.docs.starts_with("The TCP port"));
    assert!(AxumConfig::SCHEMA.source.is_some());
}
