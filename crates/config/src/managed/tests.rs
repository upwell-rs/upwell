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
    fn condition_facts() -> Vec<ConfigFactDescriptor> {
        vec![
            ConfigFactDescriptor {
                id: ConfigFactId::new("FeatureFlags", "flags", "enabled"),
                kind: ConditionScalarKind::Bool,
                source: DescriptorSource::UNKNOWN,
            },
            ConfigFactDescriptor {
                id: ConfigFactId::new("FeatureFlags", "flags", "level"),
                kind: ConditionScalarKind::Integer,
                source: DescriptorSource::UNKNOWN,
            },
        ]
    }

    fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![
            (
                ConfigFactId::new("FeatureFlags", "flags", "enabled"),
                ConditionScalar::Bool(self.enabled),
            ),
            (
                ConfigFactId::new("FeatureFlags", "flags", "level"),
                ConditionScalar::Integer(self.level as i128),
            ),
        ]
    }
}

#[test]
fn condition_fact_sources_capture_and_extract_facts() {
    let source = ConditionFactSource::of::<FeatureFlags>("flags");

    let descriptors = (source.facts.descriptors)();

    assert_eq!(descriptors.len(), 2);
    assert_eq!(descriptors[0].id.property_path, "enabled");
    assert_eq!(descriptors[1].id.property_path, "level");

    // Framework extension seams may supply either a plain value or an explicitly nested
    // shared value; both shapes must extract.
    let plain = (source.facts.scalars)(&FeatureFlags {
        enabled: true,
        level: 7,
    });
    let shared = (source.facts.scalars)(&std::sync::Arc::new(FeatureFlags {
        enabled: false,
        level: 3,
    }));

    assert!(plain.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "enabled"),
        ConditionScalar::Bool(true)
    )));
    assert!(plain.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "level"),
        ConditionScalar::Integer(7)
    )));
    assert!(shared.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "enabled"),
        ConditionScalar::Bool(false)
    )));
    assert!(shared.contains(&(
        ConfigFactId::new("FeatureFlags", "flags", "level"),
        ConditionScalar::Integer(3)
    )));
}
