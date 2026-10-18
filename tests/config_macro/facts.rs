//! Condition facts extracted from staged config values.

use super::*;

/// A config type declaring condition facts, registered against the auto-discovered
/// binding path, to prove the staged-value extraction path works end to end.
#[config(path = "flags")]
#[derive(Debug, Deserialize)]
struct FlagCfg {
    #[default = "false"]
    enabled: bool,
}

impl ConditionFacts for FlagCfg {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("FlagCfg", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: DescriptorSource::UNKNOWN,
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("FlagCfg", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

#[test]
fn registered_condition_facts_extract_from_staged_values() {
    let manager = seeded_manager("[flags]\nenabled = true\n");
    let source = ConditionFactSource::of::<FlagCfg>("flags");

    let descriptors = (source.facts.descriptors)(source.path);

    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].id.property_path, "enabled");

    let value: FlagCfg = manager.get_config::<FlagCfg>("flags").unwrap();
    let scalars = (source.facts.scalars)(source.path, &value);

    assert_eq!(
        scalars,
        vec![(
            ConfigFactId::new("FlagCfg", "flags", "enabled"),
            ConditionScalar::Bool(true)
        )]
    );
}
