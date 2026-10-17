use std::sync::Arc;

use upwell_core::condition::scope::{self, ConditionInputRejection};
use upwell_core::{
    ConditionCallback, ConditionContext, ConditionDescriptor, ConditionExpression,
    ConditionInputId, ConditionInputRead, ConditionScalar, ConditionScalarKind, ConfigBindingId,
    ConfigFactDescriptor, ConfigFactId, DependencyObservation, DescriptorSource, ProviderMappingId,
    Singleton, Transient, TypeDescriptor,
};

use super::*;
use crate::test_support::{component, no_dependencies, provider, unbuilt_factories};
use crate::{ComponentFactoryDescriptor, Dep, FromContainer, Live, ProviderOrder};

const ENABLED: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "custom.enabled");
const MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");
const SOURCE: DescriptorSource = upwell_core::descriptor_source!();

fn missing_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    vec![crate::dependency_of::<u128>(
        upwell_core::Cardinality::One,
        false,
        false,
    )]
}

fn factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", no_dependencies, true)()
}

fn missing_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", missing_dependencies, true)()
}

fn facts() -> [ConfigFactDescriptor; 2] {
    [
        ConfigFactDescriptor {
            id: ENABLED,
            kind: ConditionScalarKind::Bool,
            source: SOURCE,
        },
        ConfigFactDescriptor {
            id: MODE,
            kind: ConditionScalarKind::String,
            source: SOURCE,
        },
    ]
}

fn registry(components: Vec<ComponentDescriptor>) -> ComponentRegistry {
    ComponentRegistry {
        components,
        providers: Vec::new(),
    }
}

fn snapshot(enabled: bool, mode: &str) -> ConditionFactSnapshot {
    ConditionFactSnapshot::new([
        (ENABLED, ConditionScalar::Bool(enabled)),
        (MODE, ConditionScalar::string(mode)),
    ])
    .expect("facts are unique")
}

struct DefaultAuthenticator;
struct CustomAuthenticator;
trait Authenticator: Send + Sync {}

static CUSTOM_ENABLED: ConditionDescriptor =
    ConditionDescriptor::config_bool("custom-enabled", ENABLED);

struct ComponentA;
struct ComponentB;

#[allow(dead_code)]
fn _live_is_not_an_injectable_factory_argument(_: Live<ComponentA>) {}

mod callbacks;
mod composition;
mod config_inputs;
mod cycles;
mod incremental;
mod providers;
