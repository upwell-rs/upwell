use serde::Deserialize;
use upwell_core::{
    ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
    DependencyObservation, DescriptorSource,
};
use upwell_di::FromContainer;

use super::{Cfg, ConditionFactSource, ConditionFacts, ConfigProperties};

#[derive(Deserialize)]
struct TestConfig;

impl ConfigProperties for TestConfig {
    const NAME: &'static str = "TestConfig";
}

#[test]
fn cfg_dependency_is_generation_aware() {
    assert_eq!(
        <Cfg<TestConfig> as FromContainer>::dependency().observation,
        DependencyObservation::Live
    );
}

#[derive(Deserialize)]
struct FeatureFlags {
    enabled: bool,
    level: i64,
}

impl ConfigProperties for FeatureFlags {
    const NAME: &'static str = "FeatureFlags";
}

impl ConditionFacts for FeatureFlags {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![
            ConfigFactDescriptor {
                id: ConfigFactId::new("FeatureFlags", binding_path, "enabled"),
                kind: ConditionScalarKind::Bool,
                source: DescriptorSource::UNKNOWN,
            },
            ConfigFactDescriptor {
                id: ConfigFactId::new("FeatureFlags", binding_path, "level"),
                kind: ConditionScalarKind::Integer,
                source: DescriptorSource::UNKNOWN,
            },
        ]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![
            (
                ConfigFactId::new("FeatureFlags", binding_path, "enabled"),
                ConditionScalar::Bool(self.enabled),
            ),
            (
                ConfigFactId::new("FeatureFlags", binding_path, "level"),
                ConditionScalar::Integer(self.level as i128),
            ),
        ]
    }
}

#[test]
fn condition_fact_sources_capture_and_extract_facts() {
    let source = ConditionFactSource::of::<FeatureFlags>("flags");

    let descriptors = (source.facts.descriptors)(source.path);

    assert_eq!(descriptors.len(), 2);
    assert_eq!(descriptors[0].id.property_path, "enabled");
    assert_eq!(descriptors[1].id.property_path, "level");

    let scalars = (source.facts.scalars)(
        source.path,
        &FeatureFlags {
            enabled: true,
            level: 7,
        },
    );

    assert!(scalars.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "enabled"),
        ConditionScalar::Bool(true)
    )));
    assert!(scalars.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "level"),
        ConditionScalar::Integer(7)
    )));
}

#[test]
fn condition_fact_sources_distinguish_bindings_of_one_type() {
    let primary = ConditionFactSource::of::<FeatureFlags>("flags");
    let shadow = ConditionFactSource::of::<FeatureFlags>("shadow");

    let primary_scalars = (primary.facts.scalars)(
        primary.path,
        &FeatureFlags {
            enabled: true,
            level: 7,
        },
    );
    let shadow_scalars = (shadow.facts.scalars)(
        shadow.path,
        &FeatureFlags {
            enabled: false,
            level: 3,
        },
    );

    assert_ne!(
        primary_scalars[0].0, shadow_scalars[0].0,
        "the same fact at distinct binding paths must carry distinct ids"
    );
    assert_eq!(primary_scalars[0].0.binding_path, "flags");
    assert_eq!(shadow_scalars[0].0.binding_path, "shadow");
    assert_eq!(primary_scalars[0].1, ConditionScalar::Bool(true));
    assert_eq!(shadow_scalars[0].1, ConditionScalar::Bool(false));
}
