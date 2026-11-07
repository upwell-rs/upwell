use std::any::TypeId;
use std::sync::Arc;

use upwell_config::{ConditionFactSource, ConditionFacts, ConfigBinding, ConfigProperties};
use upwell_core::{
    Cardinality, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
    DependencyDescriptor, Scope, TypeDescriptor,
};
use upwell_di::{Component, ComponentDescriptor, Singleton};

use super::AppRegistry;
use crate::Error;
use crate::test_support::{test_component, unbuilt_factories};

#[derive(serde::Deserialize)]
struct TestConfig;

impl ConfigProperties for TestConfig {
    const NAME: &'static str = "TestConfig";
}

struct RegisteredComponent;

test_component!(RegisteredComponent, "registered");

fn config_deps() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "cfg",
        ty: TypeDescriptor::of::<TestConfig>("TestConfig"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: true,
        resolution: upwell_core::ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Live,
    }]
}

fn component() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        "needs_config",
        "NeedsConfig",
        TypeDescriptor::of::<()>("NeedsConfig"),
        Singleton::KEY,
    )
    .with_factories(unbuilt_factories!("static", config_deps))
}

#[test]
fn resolves_component_descriptor_by_concrete_type() {
    let registry = AppRegistry {
        components: vec![ComponentDescriptor::of::<RegisteredComponent>()],
        ..Default::default()
    };

    let descriptor = registry
        .resolved_component::<RegisteredComponent>()
        .expect("component resolution succeeds")
        .expect("typed component is registered");

    assert_eq!(descriptor.id, RegisteredComponent::ID);
    assert_eq!(descriptor.ty.type_id, TypeId::of::<RegisteredComponent>());
}

#[test]
fn missing_unqualified_config_binding_is_missing_not_ambiguous() {
    let registry = AppRegistry {
        components: vec![component()],
        ..Default::default()
    };

    let err = registry.validate().expect_err("config binding is missing");

    assert!(matches!(
        err,
        Error::MissingConfig {
            component,
            type_name,
            path,
        } if component == "NeedsConfig"
            && type_name.ends_with("TestConfig")
            && path == "<unqualified>"
    ));
}

#[test]
fn multiple_unqualified_config_bindings_are_ambiguous() {
    let registry = AppRegistry {
        components: vec![component()],
        providers: Vec::new(),
        config_bindings: vec![
            ConfigBinding::of::<TestConfig>("one"),
            ConfigBinding::of::<TestConfig>("two"),
        ],
        ..Default::default()
    };

    let err = registry
        .validate()
        .expect_err("config binding is ambiguous");

    assert!(matches!(
        err,
        Error::AmbiguousConfig {
            component,
            type_name,
            count: 2,
            paths,
        } if component == "NeedsConfig"
            && type_name.ends_with("TestConfig")
            && paths == "one, two"
    ));
}

#[test]
fn condition_snapshot_extracts_scalars_from_staged_values() {
    #[derive(serde::Deserialize)]
    struct FlagConfig {
        enabled: bool,
    }

    impl ConfigProperties for FlagConfig {
        const NAME: &'static str = "FlagConfig";
    }

    impl ConditionFacts for FlagConfig {
        fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
            vec![ConfigFactDescriptor {
                id: ConfigFactId::new("FlagConfig", binding_path, "enabled"),
                kind: ConditionScalarKind::Bool,
                source: upwell_core::DescriptorSource::caller(),
            }]
        }

        fn condition_scalars(
            &self,
            binding_path: &'static str,
        ) -> Vec<(ConfigFactId, ConditionScalar)> {
            vec![(
                ConfigFactId::new("FlagConfig", binding_path, "enabled"),
                ConditionScalar::Bool(self.enabled),
            )]
        }
    }

    let mut registry = AppRegistry::default();
    registry
        .condition_facts
        .push(ConditionFactSource::of::<FlagConfig>("flags"));

    let enabled = FlagConfig { enabled: true };
    let disabled = FlagConfig { enabled: false };
    let unrelated = TestConfig;

    let snapshot = registry
        .condition_snapshot(
            [
                (
                    TypeId::of::<FlagConfig>(),
                    "flags",
                    &(Arc::new(enabled) as Arc<dyn std::any::Any + Send + Sync>),
                ),
                (
                    TypeId::of::<FlagConfig>(),
                    "other",
                    &(Arc::new(disabled) as Arc<dyn std::any::Any + Send + Sync>),
                ),
                (
                    TypeId::of::<TestConfig>(),
                    "flags",
                    &(Arc::new(unrelated) as Arc<dyn std::any::Any + Send + Sync>),
                ),
            ],
            |_| Ok(None),
        )
        .expect("snapshot validates");

    registry
        .evaluate_conditions(
            <FlagConfig as ConditionFacts>::condition_facts("flags"),
            &snapshot,
        )
        .expect("the matching staged binding supplies the declared fact");
}

#[test]
fn condition_snapshot_distinguishes_two_bindings_of_one_type() {
    #[derive(serde::Deserialize)]
    struct FlagConfig {
        enabled: bool,
    }

    impl ConfigProperties for FlagConfig {
        const NAME: &'static str = "FlagConfig";
    }

    impl ConditionFacts for FlagConfig {
        fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
            vec![ConfigFactDescriptor {
                id: ConfigFactId::new("FlagConfig", binding_path, "enabled"),
                kind: ConditionScalarKind::Bool,
                source: upwell_core::DescriptorSource::caller(),
            }]
        }

        fn condition_scalars(
            &self,
            binding_path: &'static str,
        ) -> Vec<(ConfigFactId, ConditionScalar)> {
            vec![(
                ConfigFactId::new("FlagConfig", binding_path, "enabled"),
                ConditionScalar::Bool(self.enabled),
            )]
        }
    }

    let mut registry = AppRegistry::default();
    registry
        .condition_facts
        .push(ConditionFactSource::of::<FlagConfig>("flags"));
    registry
        .condition_facts
        .push(ConditionFactSource::of::<FlagConfig>("toggles"));

    let primary = FlagConfig { enabled: true };
    let shadow = FlagConfig { enabled: false };

    let snapshot = registry
        .condition_snapshot(
            [
                (
                    TypeId::of::<FlagConfig>(),
                    "flags",
                    &(Arc::new(primary) as Arc<dyn std::any::Any + Send + Sync>),
                ),
                (
                    TypeId::of::<FlagConfig>(),
                    "toggles",
                    &(Arc::new(shadow) as Arc<dyn std::any::Any + Send + Sync>),
                ),
            ],
            |_| Ok(None),
        )
        .expect("each binding contributes its own fact");

    let facts = registry
        .condition_facts
        .iter()
        .flat_map(|source| (source.facts.descriptors)(source.path));

    registry
        .evaluate_conditions(facts, &snapshot)
        .expect("both bindings evaluate without duplicate facts");
}

trait Greeter: Send + Sync {}

struct English;

impl Greeter for English {}

crate::test_support::test_component!(English, "english", provides dyn Greeter);

static ENGLISH_GREETER: upwell_di::ProviderDescriptor =
    upwell_di::ProviderDescriptor::of::<English, dyn Greeter>("dyn Greeter");

#[test]
fn a_provider_mapping_registered_twice_is_kept_once() {
    let mut registry = AppRegistry::default();

    registry.add_provider(ENGLISH_GREETER);
    registry.add_provider(ENGLISH_GREETER);
    registry.add_provider(ENGLISH_GREETER.qualifier("en"));

    assert_eq!(
        registry
            .providers
            .iter()
            .map(|provider| provider.qualifier)
            .collect::<Vec<_>>(),
        ["english", "en"],
        "an identical mapping is kept once; a different qualifier is a distinct mapping"
    );
}
