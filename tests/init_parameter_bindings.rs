//! `#[init]` parameters take the same binding attributes as `#[component]` fields:
//! `#[config("path")]` and `#[qualifier = ".."]` end up on the factory's dependency edges.
#![allow(dead_code)]

use std::sync::Arc;

use upwell::{Cfg, ComponentFactories, component, config, injectable, methods};

#[config]
#[derive(serde::Deserialize)]
struct Replica {
    host: String,
}

#[injectable]
trait Backend: Send + Sync {}

#[component(provide = dyn Backend, qualifier = "primary")]
struct PrimaryBackend;

impl Backend for PrimaryBackend {}

/// A component whose constructor binds a config path and a provider qualifier.
#[component]
struct Router {
    #[default]
    host: String,
}

#[methods]
impl Router {
    #[init]
    fn build(
        #[config("replicas.eu")] replica: Cfg<Replica>,
        #[qualifier = "primary"] _backend: Arc<dyn Backend>,
        _plain: Arc<PrimaryBackend>,
    ) -> Self {
        Self {
            host: replica.get().host.clone(),
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

    assert_eq!(dependencies.len(), 3);
    assert!(dependencies[0].config);
    assert_eq!(dependencies[0].qualifier, Some("replicas.eu"));
    assert!(!dependencies[1].config);
    assert_eq!(dependencies[1].qualifier, Some("primary"));
    assert_eq!(dependencies[2].qualifier, None);
}
