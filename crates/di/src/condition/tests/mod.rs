use std::sync::Arc;

use upwell_core::{
    AvailabilityConditionCallback, AvailabilityConditionContext, AvailabilityTarget,
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigConditionCallback, ConfigConditionContext, ConfigFactDescriptor, ConfigFactId,
    DependencyObservation, DescriptorSource, ProviderMappingId, Singleton, Transient,
    TypeDescriptor,
};

use super::*;
use crate::test_support::{no_dependencies, planned, planned_provider, unbuilt_factories};
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

fn component<T: 'static>(
    id: &'static str,
    condition: Option<&'static ConditionDescriptor>,
) -> ComponentDescriptor {
    ComponentDescriptor {
        condition,
        ..planned(id, TypeDescriptor::of::<T>(id), factory)
    }
}

fn provider<T: 'static>(
    trait_ty: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        primary,
        ..planned_provider(
            trait_ty,
            TypeDescriptor::of::<T>(std::any::type_name::<T>()),
            qualifier,
        )
    }
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
mod cycles;
mod incremental;
mod providers;
