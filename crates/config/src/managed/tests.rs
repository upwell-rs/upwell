use serde::Deserialize;
use upwell_core::DependencyObservation;
use upwell_di::FromContainer;

use super::{Cfg, ConfigProperties};

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
