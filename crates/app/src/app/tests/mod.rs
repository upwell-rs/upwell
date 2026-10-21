use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use upwell_config::{
    ConditionFactSource, ConditionFacts, ConfigBinding, ConfigManager, ConfigProperties,
    ConfigStore, Toml,
};
use upwell_core::{
    ConditionCallback, ConditionContext, ConditionDescriptor, ConditionInputId, ConditionPredicate,
    ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId, ResolverSet,
};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, Injectable, RootResolver, ScopeContainer,
    ScopeRegistry, factories, root_resolver_descriptor,
};
use upwell_dirs::{Config as ConfigDir, Dir, DirectoriesManager};
use upwell_hooks::HookManager;

use super::App;
use crate::builtins::RuntimeReloader;
use crate::test_support::test_component;
use crate::{
    AppRegistry, AppRuntime, LoggingConfig, PreBuildContext, PreparedProtocol, ProtocolDefinition,
    ProtocolRuntime, ScopeTopology, ShutdownSignal, ValidationContext,
};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static PRE_BUILD_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROTOCOL_BUILD_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Serializes the tests sharing the boundary counters: `BoundaryProtocol`'s preparation
/// and runtime construction assert exact process-global counts, so concurrent runs
/// under the default `cargo test` parallelism must not interleave. Await-safe, so no
/// std guard ever crosses an await.
static BOUNDARY_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Component whose factory records the construction boundary.
struct BoundaryComponent;

test_component!(BoundaryComponent, "boundary_component");

/// Prebuilt component contributed by protocol preparation.
struct SeededComponent;

test_component!(SeededComponent, "seeded_component");

/// A user-supplied pre-built component registered through `with_component`.
struct UserProvided;

static TYPED_USER_PROVIDED: ComponentDescriptor = ComponentDescriptor::of::<UserProvided>();

test_component!(UserProvided, "user_provided");

async fn boundary_component() -> BoundaryComponent {
    FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

    BoundaryComponent
}

static BOUNDARY_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<BoundaryComponent>()
    .with_factories(factories![default "static" => boundary_component])
    .without_retention();

/// Protocol definition recording preparation calls.
#[derive(Default)]
struct BoundaryProtocol;

/// Validated protocol state retained before runtime construction.
struct PreparedBoundaryProtocol;

/// Protocol runtime produced after the component graph is constructed.
struct BoundaryRuntime;

impl ProtocolRuntime for BoundaryRuntime {
    type Error = crate::Error;
}

impl ProtocolDefinition for BoundaryProtocol {
    type Prepared = PreparedBoundaryProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId = upwell_core::namespaced_id!(crate::ProtocolId, "test/boundary");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        PRE_BUILD_CALLS.fetch_add(1, Ordering::SeqCst);

        assert_eq!(context.name(), "prepare-boundary-test");
        assert!(
            context
                .resolved_components()
                .iter()
                .any(|component| component.id == BoundaryComponent::ID)
        );
        assert!(
            context
                .resolved_components()
                .iter()
                .any(|component| component.id == SeededComponent::ID)
        );
        assert_eq!(
            context
                .config::<LoggingConfig>("logging")
                .expect("protocol config binding is finalized")
                .snapshot()
                .level,
            "debug"
        );

        Ok(PreparedBoundaryProtocol)
    }

    fn pre_build(&mut self, context: &mut PreBuildContext<'_>) -> Result<(), Self::Error> {
        context.component_descriptor(&BOUNDARY_COMPONENT);
        context.with_component(SeededComponent);
        context.config::<LoggingConfig>("logging");

        Ok(())
    }
}

impl PreparedProtocol for PreparedBoundaryProtocol {
    type Runtime = BoundaryRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &AppRuntime) -> Result<Self::Runtime, Self::Error> {
        assert_eq!(
            FACTORY_CALLS.load(Ordering::SeqCst),
            1,
            "root components must be constructed before the protocol runtime"
        );
        PROTOCOL_BUILD_CALLS.fetch_add(1, Ordering::SeqCst);

        Ok(BoundaryRuntime)
    }

    #[cfg(feature = "tooling")]
    fn tooling(&self, contributions: &mut crate::ToolingContributions) {
        contributions.display(crate::ResourceDisplay {
            label: Some(String::from("Boundary test protocol")),
            ..Default::default()
        });
    }
}

mod boundary;
mod conditions;
mod root_order;
mod snapshots;
