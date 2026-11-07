//! `#[init]` parameters take the same binding attributes as `#[component]` fields:
//! `#[config("path")]` and `#[qualifier = ".."]` end up on the factory's dependency edges and
//! select the config binding and provider when the component is built.
#![allow(dead_code)]

use std::sync::Arc;

use upwell::{App, Cfg, ComponentFactories, component, config, injectable, methods};

/// A config section bound at both `replicas.eu` and `replicas.us`.
#[config]
#[derive(serde::Deserialize)]
struct Replica {
    host: String,
}

/// A backend selected by qualifier.
#[injectable]
trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The backend provided under the `primary` qualifier.
#[component(provide = dyn Backend, qualifier = "primary")]
struct PrimaryBackend;

impl Backend for PrimaryBackend {
    fn name(&self) -> &'static str {
        "primary"
    }
}

/// The backend provided under the `secondary` qualifier.
#[component(provide = dyn Backend, qualifier = "secondary")]
struct SecondaryBackend;

impl Backend for SecondaryBackend {
    fn name(&self) -> &'static str {
        "secondary"
    }
}

/// A component whose constructor binds a config path and a provider qualifier.
#[component]
struct Router {
    #[default]
    host: String,
    #[default]
    us_host: String,
    #[default]
    backend: String,
}

#[methods]
impl Router {
    #[init]
    fn build(
        #[config("replicas.eu")] replica: Cfg<Replica>,
        #[config("replicas.us")] us_replica: Cfg<Replica>,
        #[qualifier = "secondary"] backend: Arc<dyn Backend>,
        _plain: Arc<PrimaryBackend>,
    ) -> Self {
        Self {
            host: replica.get().host.clone(),
            us_host: us_replica.get().host.clone(),
            backend: backend.name().to_string(),
        }
    }
}

#[test]
fn bound_parameters_report_their_path_and_qualifier() {
    let factory = Router::factories()
        .iter()
        .find(|factory| factory.id == "init:build")
        .expect("the init is registered");
    let dependencies = (factory.dependencies)();

    assert_eq!(dependencies.len(), 4);
    assert!(dependencies[0].config);
    assert_eq!(dependencies[0].qualifier, Some("replicas.eu"));
    assert!(dependencies[1].config);
    assert_eq!(dependencies[1].qualifier, Some("replicas.us"));
    assert!(!dependencies[2].config);
    assert_eq!(dependencies[2].qualifier, Some("secondary"));
    assert_eq!(dependencies[3].qualifier, None);
}

#[tokio::test]
async fn bound_parameters_select_the_config_path_and_the_qualified_provider() {
    let config = upwell::ConfigManager::<upwell::config::Toml>::from_str(
        "[replicas.eu]\nhost = \"eu-host\"\n[replicas.us]\nhost = \"us-host\"\n",
    )
    .expect("the config parses");
    let app = App::<()>::builder("init-bindings")
        .auto_discover()
        .config::<Replica>("replicas.eu")
        .config::<Replica>("replicas.us")
        .config_source(config)
        .build()
        .await
        .expect("the application builds");

    let router = app.container().get::<Router>().expect("the router builds");

    assert_eq!(
        router.host, "eu-host",
        "the config comes from the bound path"
    );
    assert_eq!(
        router.us_host, "us-host",
        "the second binding comes from its own path"
    );
    assert_eq!(
        router.backend, "secondary",
        "the qualifier selects the provider"
    );
}
