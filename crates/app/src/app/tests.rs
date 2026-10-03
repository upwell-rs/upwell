use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_config::{ConfigManager, Toml};
use upwell_core::TypeDescriptor;
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, Injectable, Singleton,
};

use super::App;
use crate::{
    AppRegistry, AppRuntime, LoggingConfig, PreBuildContext, PreparedProtocol, ProtocolDefinition,
    ProtocolRuntime, ScopeTopology, ValidationContext,
};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static PRE_BUILD_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROTOCOL_BUILD_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Component whose factory records the construction boundary.
struct BoundaryComponent;

impl Component for BoundaryComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "boundary_component";
    const NAME: &'static str = "BoundaryComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

/// Prebuilt component contributed by protocol preparation.
struct SeededComponent;

impl Component for SeededComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "seeded_component";
    const NAME: &'static str = "SeededComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn construct_boundary_component(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<BoundaryComponent>(BoundaryComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(BoundaryComponent))),
        })
    })
}

fn no_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

static BOUNDARY_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: construct_boundary_component,
    dependencies: no_dependencies,
    default: true,
}];

fn boundary_factories() -> &'static [ComponentFactoryDescriptor] {
    &BOUNDARY_FACTORIES
}

static BOUNDARY_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: BoundaryComponent::ID,
    name: BoundaryComponent::NAME,
    ty: TypeDescriptor::of::<BoundaryComponent>(BoundaryComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories: boundary_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

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

#[tokio::test]
async fn prepare_validates_without_constructing_components_or_protocol() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    PRE_BUILD_CALLS.store(0, Ordering::SeqCst);
    PROTOCOL_BUILD_CALLS.store(0, Ordering::SeqCst);

    let prepared = App::<BoundaryProtocol>::builder("prepare-boundary-test")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [logging]
                    level = "debug"
                    format = "compact"
                    ansi = false
                "#,
            )
            .expect("test config parses"),
        )
        .prepare()
        .expect("application prepares");

    assert_eq!(PRE_BUILD_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(PROTOCOL_BUILD_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::any::type_name_of_val(prepared.protocol()),
        std::any::type_name::<PreparedBoundaryProtocol>()
    );

    let app = prepared.build().await.expect("prepared application builds");

    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(PROTOCOL_BUILD_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::any::type_name_of_val(app.protocol()),
        std::any::type_name::<BoundaryRuntime>()
    );
    assert!(app.container().get::<BoundaryComponent>().is_some());
    assert!(app.container().get::<SeededComponent>().is_some());
}

#[test]
#[cfg(feature = "tooling")]
fn retained_tooling_construction_plan_equals_the_runtime_build_plan() {
    let prepared = App::<BoundaryProtocol>::builder("prepare-boundary-test")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [logging]
                    level = "debug"
                    format = "compact"
                    ansi = false
                "#,
            )
            .expect("test config parses"),
        )
        .prepare()
        .expect("application prepares");
    let runtime = prepared
        .root_order
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let tooling = prepared
        .tooling_snapshot()
        .root_plan()
        .iter()
        .map(|entry| entry.descriptor.id)
        .collect::<Vec<_>>();

    assert_eq!(tooling, runtime);
}
