#![allow(dead_code)]

use serde::Deserialize;
use serde_json::Value;
use upwell_config::{ConfigManager, ConfigProperties, Toml};
use upwell_macros::config;
use upwell_tooling_schema::ToolingDocument;

use super::*;
use crate::{App, AppBuilder};

/// HTTP listener settings.
#[config(upwell = ::upwell_config)]
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct HttpConfig {
    /// The listen port.
    #[default = "8080"]
    port: u16,

    /// The public host name.
    public_host: String,

    /// Optional TLS settings.
    tls: Option<TlsConfig>,

    /// The selected backend.
    backend: Backend,
}

/// TLS settings.
#[config(upwell = ::upwell_config)]
#[derive(Deserialize)]
struct TlsConfig {
    /// The certificate path.
    cert: std::path::PathBuf,
}

/// A config enum.
#[config(upwell = ::upwell_config)]
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Backend {
    /// In-memory storage.
    #[default]
    Memory,

    /// A database.
    Database {
        /// The connection URL.
        url: String,
    },
}

/// Plain serde pool settings from another crate, with a strictly parsed field first.
#[derive(Deserialize)]
struct Pool {
    bind: std::net::IpAddr,
    max_size: u32,
}

/// Pool settings wrapped by a transparent config.
#[config(upwell = ::upwell_config)]
#[derive(Deserialize)]
#[serde(transparent)]
struct PoolConfig(Pool);

/// A config type implemented by hand, without `#[config]`.
#[derive(Deserialize)]
struct ManualConfig {
    retries: u8,
    endpoints: Vec<String>,
}

impl ConfigProperties for ManualConfig {
    const NAME: &'static str = "ManualConfig";
}

/// Values satisfying every fixture binding, so preparation binds them all.
const FIXTURE_CONFIG: &str = r#"
[http]
public-host = "example.test"
backend = { kind = "memory" }

[server]
retries = 3
endpoints = ["a"]

[server.http.tls]
cert = "/etc/cert.pem"
"#;

fn project(configure: impl FnOnce(AppBuilder<()>) -> AppBuilder<()>) -> ToolingDocument {
    let source = ConfigManager::<Toml>::from_str(FIXTURE_CONFIG).expect("fixture config parses");
    let builder = App::<()>::builder("config-schema").config_source(source);

    configure(builder)
        .prepare()
        .expect("config schema fixture prepares")
        .tooling_document()
        .expect("config schema fixture projects")
}

fn tree(document: &ToolingDocument) -> &Value {
    &document.facets[CONFIG_SCHEMA_FACET].value
}

#[test]
fn declared_configs_render_fields_with_docs_defaults_and_sources() {
    let schema = binding_schema(&ConfigBinding::of::<HttpConfig>("http"));
    let port = &schema["properties"]["port"];
    let field = &port["x-upwell-field"];

    assert_eq!(schema["description"], "HTTP listener settings.");
    assert_eq!(port["description"], "The listen port.");
    assert_eq!(port["default"], "8080");
    assert_eq!(port["x-upwell-template"], true);
    assert_eq!(field["declared-type"], "u16");
    assert_eq!(field["rust-type"], "u16");
    assert!(
        field["source"]["file"]
            .as_str()
            .is_some_and(|file| std::path::Path::new(file).ends_with("config_schema/tests.rs"))
    );
    assert!(
        field["source"]["rust_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("tests::HttpConfig::port"))
    );
    assert_eq!(
        schema["required"],
        serde_json::json!(["public-host", "backend"])
    );
}

#[test]
fn nested_configs_and_enums_carry_their_own_schemas() {
    let schema = binding_schema(&ConfigBinding::of::<HttpConfig>("http"));
    let tls = &schema["properties"]["tls"]["anyOf"][0];
    let backend = &schema["properties"]["backend"];
    let database = &backend["oneOf"][1];

    assert_eq!(tls["description"], "TLS settings.");
    assert_eq!(
        tls["properties"]["cert"]["description"],
        "The certificate path."
    );
    assert_eq!(backend["x-upwell-default-variant"], "memory");
    assert_eq!(database["properties"]["kind"]["const"], "database");
    assert_eq!(database["description"], "A database.");
    assert_eq!(
        database["properties"]["url"]["description"],
        "The connection URL."
    );
}

#[test]
fn transparent_configs_render_their_wrapped_shape_with_their_own_docs() {
    let schema = binding_schema(&ConfigBinding::of::<PoolConfig>("pool"));

    assert_eq!(
        schema["description"],
        "Pool settings wrapped by a transparent config."
    );
    assert_eq!(schema["properties"]["bind"]["type"], "string");
    assert_eq!(
        schema["properties"]["max_size"]["anyOf"][0]["type"],
        "integer"
    );
}

#[test]
fn hand_written_configs_render_their_traced_shape() {
    let schema = binding_schema(&ConfigBinding::of::<ManualConfig>("manual"));

    assert_eq!(schema["x-upwell-traced"], true);
    assert_eq!(
        schema["properties"]["retries"]["anyOf"][0]["type"],
        "integer"
    );
    assert_eq!(schema["properties"]["endpoints"]["type"], "array");
}

#[test]
fn the_document_nests_bindings_by_path_and_describes_templating() {
    let document = project(|builder| {
        builder
            .config::<TlsConfig>("server.http.tls")
            .config::<ManualConfig>("server")
    });
    let tree = tree(&document);
    let server = &tree["properties"]["server"];

    assert_eq!(
        server["properties"]["retries"]["anyOf"][0]["type"],
        "integer"
    );
    assert_eq!(
        server["properties"]["http"]["properties"]["tls"]["description"],
        "TLS settings."
    );
    assert!(
        tree["x-upwell-templating"]["config-paths"]
            .as_array()
            .is_some_and(|paths| paths.contains(&serde_json::json!("server.http.tls")))
    );
    assert!(tree["$defs"][json::TEMPLATE_DEF].is_object());
}

#[test]
fn binding_resources_carry_their_schema_facet_and_type_source() {
    let document = project(|builder| builder.config::<HttpConfig>("http"));
    let id = config_binding_id(std::any::type_name::<HttpConfig>(), "http");
    let resource = document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .expect("the binding resource exists");
    let facet = &resource.facets[&format!("{id}/{CONFIG_BINDING_SCHEMA_FACET}")];
    let source = resource
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.source.as_ref())
        .expect("a runtime binding still names its type's declaration");

    assert_eq!(facet.value["path"], "http");
    assert_eq!(
        facet.value["schema"]["properties"]["public-host"]["description"],
        "The public host name."
    );
    assert!(
        source
            .rust_path
            .as_deref()
            .is_some_and(|path| path.ends_with("tests::HttpConfig"))
    );
}

#[test]
fn bindings_sharing_a_path_must_all_hold() {
    let mut root = Map::new();
    let first = json!({ "type": "object", "required": ["a"], "properties": { "a": {} } });
    let second = json!({ "oneOf": [{ "type": "object" }], "x-upwell-rust-type": "Second" });

    insert_at_path(&mut root, "shared", first.clone());
    insert_at_path(&mut root, "shared", second.clone());

    assert_eq!(root["shared"], json!({ "allOf": [first, second] }));
}

#[test]
fn deeper_paths_are_not_nested_under_a_scalar_binding() {
    let mut root = Map::new();
    let scalar = json!({ "type": "string" });

    insert_at_path(&mut root, "name", scalar.clone());
    insert_at_path(&mut root, "name.inner", json!({ "type": "integer" }));
    insert_at_path(&mut root, "parent.child", json!({ "type": "integer" }));

    assert_eq!(root["name"], scalar);
    assert_eq!(
        root["parent"]["properties"]["child"],
        json!({ "type": "integer" })
    );
}
