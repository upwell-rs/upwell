use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_config::{ConditionFacts, ConfigManager, ConfigProperties, ConfigReloader, Toml};
use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, ResolverSet, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, Injectable, RootResolver, ScopeContainer, ScopeRegistry, Singleton,
    root_resolver_descriptor,
};
use upwell_dirs::{Config as ConfigDir, Dir, DirectoriesManager};
use upwell_hooks::HookManager;

use super::App;
use crate::{
    AppRegistry, AppRuntime, LoggingConfig, PreBuildContext, PreparedProtocol, ProtocolDefinition,
    ProtocolRuntime, ScopeTopology, ShutdownSignal, ValidationContext,
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

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// A user-supplied pre-built component registered through `with_component`.
struct UserProvided;

static TYPED_USER_PROVIDED: ComponentDescriptor = ComponentDescriptor::of::<UserProvided>();

impl Component for UserProvided {
    type Handle = Arc<Self>;

    const ID: &'static str = "user_provided";
    const NAME: &'static str = "UserProvided";

    fn into_handle(self) -> Arc<Self> {
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

#[test]
fn framework_singletons_are_snapshot_capable_and_user_prebuilts_are_not() {
    assert!(
        super::SHUTDOWN_HANDLE_DESCRIPTOR
            .generation_snapshot
            .is_some(),
        "the shutdown handle must be retainable across generations"
    );
    assert!(
        super::CONFIG_RELOADER_DESCRIPTOR
            .generation_snapshot
            .is_some(),
        "the config reloader must be retainable across generations"
    );
    assert!(
        super::HOOK_MANAGER_DESCRIPTOR.generation_snapshot.is_none(),
        "the hook manager catalog and resolver routing stay generation-local"
    );
    assert!(
        root_resolver_descriptor().generation_snapshot.is_none(),
        "the root resolver stays generation-local"
    );

    let mut registry = AppRegistry::default();
    let mut instances = Vec::new();
    let dirs =
        DirectoriesManager::from_path(std::env::temp_dir().join("upwell-app-descriptor-policy"));

    super::seed_directories(&dirs, &mut registry, &mut instances);

    for descriptor in &registry.components {
        assert!(
            descriptor.generation_snapshot.is_some(),
            "directories descriptor '{}' must be snapshot-capable",
            descriptor.id
        );
    }

    let builder =
        App::<BoundaryProtocol>::builder("descriptor-policy").with_component(UserProvided);
    let seeded = builder
        .registry
        .components
        .last()
        .expect("with_component registers a descriptor");

    assert_eq!(seeded.id, UserProvided::ID);
    assert!(
        seeded.generation_snapshot.is_none(),
        "user pre-built instances have no transition contract"
    );
}

#[tokio::test]
async fn candidate_metadata_cannot_upgrade_user_prebuilt_provenance() {
    let app = App::<BoundaryProtocol>::builder("prepare-boundary-test")
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
        .component_descriptor(&TYPED_USER_PROVIDED)
        .with_component(UserProvided)
        .build()
        .await
        .expect("app builds");

    let error = app
        .container()
        .snapshot_singleton(TYPED_USER_PROVIDED)
        .expect_err("the active user seed remains non-retainable");

    assert!(matches!(
        error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));
}

#[tokio::test]
async fn framework_singletons_snapshot_out_of_a_built_root() {
    let shutdown = ShutdownSignal::new();
    let hooks = HookManager::new(Vec::new());
    let manager = ConfigManager::<Toml>::from_str(
        r#"
            [logging]
            level = "debug"
            format = "compact"
            ansi = false
        "#,
    )
    .expect("test config parses")
    .into_dynamic();
    let reloader = ConfigReloader::new(manager, Vec::new(), hooks.clone());
    let dirs = DirectoriesManager::from_path(std::env::temp_dir().join("upwell-app-snapshot-test"));

    let descriptors = [
        super::SHUTDOWN_HANDLE_DESCRIPTOR,
        super::CONFIG_RELOADER_DESCRIPTOR,
        super::HOOK_MANAGER_DESCRIPTOR,
        ComponentDescriptor::of::<DirectoriesManager>(),
        ComponentDescriptor::of::<Dir<ConfigDir>>(),
        root_resolver_descriptor(),
    ];
    let seeds = vec![
        BoxedComponent {
            ty: descriptors[0].ty,
            value: Box::new(shutdown.handle()),
        },
        BoxedComponent {
            ty: descriptors[1].ty,
            value: Box::new(Injectable::into_stored(reloader.clone())),
        },
        BoxedComponent {
            ty: descriptors[2].ty,
            value: Box::new(Injectable::into_stored(hooks.clone())),
        },
        BoxedComponent {
            ty: descriptors[3].ty,
            value: Box::new(dirs.clone()),
        },
        BoxedComponent {
            ty: descriptors[4].ty,
            value: Box::new(dirs.dir::<ConfigDir>()),
        },
        BoxedComponent {
            ty: descriptors[5].ty,
            value: Box::new(Injectable::into_stored(RootResolver::new())),
        },
    ];
    let components = descriptors
        .iter()
        .map(|descriptor| (descriptor.ty.type_id, *descriptor))
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, Vec::new(), HashMap::new())
            .expect("framework registry validates"),
    );
    let root = ScopeContainer::build_root(&descriptors, seeds, ResolverSet::new(), registry)
        .await
        .expect("framework root builds");

    for descriptor in [
        descriptors[0],
        descriptors[1],
        descriptors[3],
        descriptors[4],
    ] {
        root.snapshot_singleton(descriptor)
            .expect("framework singletons are snapshot-capable");
    }

    let hook_error = root
        .snapshot_singleton(descriptors[2])
        .expect_err("the hook manager is generation-local");

    assert!(matches!(
        hook_error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));

    let error = root
        .snapshot_singleton(root_resolver_descriptor())
        .expect_err("the root resolver is not snapshot-capable");

    assert!(matches!(
        error,
        upwell_di::Error::SnapshotUnavailable { .. }
    ));
}

const FLAG_ENABLED: ConfigFactId = ConfigFactId::new("test::FlagConfig", "flags", "enabled");

static FLAG_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "flag-enabled",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigBool(FLAG_ENABLED),
};

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
            id: ConfigFactId::new("test::FlagConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("test::FlagConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

/// Factory-backed singleton whose availability keys on the `flags.enabled` fact.
struct FlagComponent;

impl Component for FlagComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "flag_component";
    const NAME: &'static str = "FlagComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn construct_flag_component(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<FlagComponent>(FlagComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(FlagComponent))),
        })
    })
}

static FLAG_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: construct_flag_component,
    dependencies: no_dependencies,
    default: true,
}];

fn flag_factories() -> &'static [ComponentFactoryDescriptor] {
    &FLAG_FACTORIES
}

static FLAG_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: FlagComponent::ID,
    name: FlagComponent::NAME,
    ty: TypeDescriptor::of::<FlagComponent>(FlagComponent::NAME),
    scope: &Singleton,
    condition: Some(&FLAG_CONDITION),
    factories: flag_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

async fn build_flag_app(enabled: bool) -> crate::Result<App<()>> {
    let source = if enabled {
        "[flags]\nenabled = true\n"
    } else {
        "[flags]\nenabled = false\n"
    };

    App::<()>::builder("conditional-fact-test")
        .config_source(ConfigManager::<Toml>::from_str(source).expect("test config parses"))
        .config::<FlagConfig>("flags")
        .condition_facts::<FlagConfig>("flags")
        .component_descriptor(&FLAG_COMPONENT)
        .build()
        .await
}

#[tokio::test]
async fn initial_build_excludes_components_whose_config_fact_is_false() {
    let app = build_flag_app(false).await.expect("disabled app builds");
    let view = app.runtime().view();

    assert!(
        !view
            .resolved_components()
            .iter()
            .any(|component| component.id == FlagComponent::ID)
    );
    assert!(app.container().get::<FlagComponent>().is_none());
    assert_eq!(
        view.condition()
            .evaluation()
            .evaluation()
            .component_eligible(FlagComponent::ID),
        Some(false)
    );
}

#[tokio::test]
async fn initial_build_includes_components_whose_config_fact_is_true() {
    let app = build_flag_app(true).await.expect("enabled app builds");
    let view = app.runtime().view();

    assert!(
        view.resolved_components()
            .iter()
            .any(|component| component.id == FlagComponent::ID)
    );
    assert!(app.container().get::<FlagComponent>().is_some());
    assert_eq!(
        view.condition()
            .evaluation()
            .evaluation()
            .component_eligible(FlagComponent::ID),
        Some(true)
    );
}
