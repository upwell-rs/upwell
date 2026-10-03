use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use upwell_config::{ConfigManager, Toml};
use upwell_config::{ConfigProperties, ConfigReload};
use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, StaticScope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, Injectable, ProviderDescriptor, ProviderOrder,
    ProviderOrderDirection, Singleton,
};
use upwell_tooling_schema::{
    BinaryTargetIdentity, DocumentIdentity, PackageIdentity, ProbeEnvelope, RelationshipKind,
    SourceLocation,
};

use crate::test_support::TempFixture;
use crate::{
    App, AppRegistry, CompositionDiagnostic, CompositionDiagnostics, InstallationOrigin,
    InstallationProvenance, Plugin, PluginContributions, PluginId, PluginRelation, PluginSlotId,
    PreparedProtocol, ProtocolDefinition, ProtocolPluginRegistrar, ProtocolRuntime, RelationKind,
    RelationTarget, ScopeBoundary, ScopeParent, ScopeTopology, ToolingEndpoint,
    ToolingRelationshipKind, ValidationContext,
};
use upwell_hooks::{HookCall, HookKind};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static TOOLING_CALLS: AtomicUsize = AtomicUsize::new(0);
static FACTORY_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static DEPENDENCY_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_DEPENDENCY_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROJECTION_CALLBACK_POISONED: AtomicBool = AtomicBool::new(false);

const DIAGNOSTIC_CONSUMER_SCOPE_ID: upwell_core::ScopeId =
    upwell_core::namespaced_id!(upwell_core::ScopeId, "test/diagnostic-consumer");
const DIAGNOSTIC_DEPENDENCY_SCOPE_ID: upwell_core::ScopeId =
    upwell_core::namespaced_id!(upwell_core::ScopeId, "test/diagnostic-dependency");

struct DiagnosticConsumerScope;

impl StaticScope for DiagnosticConsumerScope {
    const ID: upwell_core::ScopeId = DIAGNOSTIC_CONSUMER_SCOPE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Diagnostic Consumer";
}

struct DiagnosticDependencyScope;

impl StaticScope for DiagnosticDependencyScope {
    const ID: upwell_core::ScopeId = DIAGNOSTIC_DEPENDENCY_SCOPE_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "Diagnostic Dependency";
}

struct DiagnosticConsumer;
struct DiagnosticDependency;

/// Trait requested by the topology-aware unreachable-provider fixture.
trait DiagnosticProvider: Send + Sync {}

/// Consumer of a trait provider registered in an inaccessible sibling scope.
struct DiagnosticProviderConsumer;

/// Trait provider registered in an inaccessible sibling scope.
struct DiagnosticProviderComponent;

impl DiagnosticProvider for DiagnosticProviderComponent {}

fn diagnostic_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "DiagnosticDependency",
        ty: TypeDescriptor::of::<DiagnosticDependency>("DiagnosticDependency"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn diagnostic_provider_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "DiagnosticProvider",
        ty: TypeDescriptor::of::<dyn DiagnosticProvider>("DiagnosticProvider"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

static DIAGNOSTIC_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "diagnostic-consumer",
        construct,
        dependencies: diagnostic_dependency,
        default: false,
    }];

fn diagnostic_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &DIAGNOSTIC_CONSUMER_FACTORIES
}

static DIAGNOSTIC_PROVIDER_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "diagnostic-provider-consumer",
        construct,
        dependencies: diagnostic_provider_dependency,
        default: false,
    }];

fn diagnostic_provider_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &DIAGNOSTIC_PROVIDER_CONSUMER_FACTORIES
}

static DIAGNOSTIC_CONSUMER: ComponentDescriptor = ComponentDescriptor {
    id: "stable-consumer-id",
    name: "Friendly Consumer Name",
    ty: TypeDescriptor::of::<DiagnosticConsumer>("DiagnosticConsumer"),
    scope: &DiagnosticConsumerScope,
    condition: None,
    factories: diagnostic_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static DIAGNOSTIC_DEPENDENCY: ComponentDescriptor = ComponentDescriptor {
    id: "stable-dependency-id",
    name: "Friendly Dependency Name",
    ty: TypeDescriptor::of::<DiagnosticDependency>("DiagnosticDependency"),
    scope: &DiagnosticDependencyScope,
    condition: None,
    factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static DIAGNOSTIC_PROVIDER_CONSUMER: ComponentDescriptor = ComponentDescriptor {
    id: "stable-provider-consumer-id",
    name: "Friendly Provider Consumer",
    ty: TypeDescriptor::of::<DiagnosticProviderConsumer>("DiagnosticProviderConsumer"),
    scope: &DiagnosticConsumerScope,
    condition: None,
    factories: diagnostic_provider_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static DIAGNOSTIC_PROVIDER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: "stable-provider-component-id",
    name: "Friendly Provider Component",
    ty: TypeDescriptor::of::<DiagnosticProviderComponent>("DiagnosticProviderComponent"),
    scope: &DiagnosticDependencyScope,
    condition: None,
    factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

fn erase_diagnostic_provider(_: &BoxedComponent) -> BoxedComponent {
    unreachable!("provider validation never erases components")
}

static DIAGNOSTIC_PROVIDER: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn DiagnosticProvider>("DiagnosticProvider"),
    concrete_ty: TypeDescriptor::of::<DiagnosticProviderComponent>("DiagnosticProviderComponent"),
    qualifier: "sibling",
    primary: false,
    priority: 0,
    ordering: &[],
    erase: erase_diagnostic_provider,
};

static DIAGNOSTIC_SCOPE_BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new(&DiagnosticConsumerScope, ScopeParent::Root),
    ScopeBoundary::new(&DiagnosticDependencyScope, ScopeParent::Root),
];

#[derive(Default)]
struct DiagnosticFailureProtocol;

impl ProtocolDefinition for DiagnosticFailureProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "test/diagnostic-failure");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&DIAGNOSTIC_SCOPE_BOUNDARIES);

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

#[derive(Default)]
struct DiagnosticProviderFailureProtocol;

impl ProtocolDefinition for DiagnosticProviderFailureProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "test/diagnostic-provider-failure");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&DIAGNOSTIC_SCOPE_BOUNDARIES);

    fn register(&self, registry: &mut AppRegistry) {
        registry.providers.push(DIAGNOSTIC_PROVIDER);
    }

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

#[test]
fn typed_config_failures_emit_safe_diagnostics_without_source_display() {
    let error = crate::Error::Config(upwell_config::ConfigError::Substitution {
        path: String::from("service.token"),
        source: upwell_config::TemplateError::Bare(upwell_config::TemplateErrorKind::Message(
            String::from("resolved secret=probe-secret"),
        )),
    });
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Configure,
        error,
    ))
    .failure();
    let envelope = ProbeEnvelope::failure(probe_identity(), failure);
    let json = envelope.to_json().expect("typed failure serializes");

    assert!(json.contains("upwell/tooling-config-substitution"));
    assert!(json.contains("config:service.token"));
    assert!(!json.contains("probe-secret"));
}

#[test]
fn arbitrary_setup_and_plugin_error_displays_never_enter_failure_envelopes() {
    let secret = "secret bearer-token=probe-secret";
    let setup = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Setup,
        std::io::Error::other(secret),
    ))
    .failure();
    let plugin = super::ToolingProbeError::PluginCatalog(crate::Error::Other(Box::new(
        std::io::Error::other(secret),
    )))
    .failure();

    for failure in [setup, plugin] {
        let json = ProbeEnvelope::failure(probe_identity(), failure)
            .to_json()
            .expect("typed failure serializes");

        assert!(!json.contains("probe-secret"));
        assert!(!json.contains("bearer-token"));
    }
}

#[test]
fn plugin_composition_failures_preserve_stable_plugin_and_installation_identities() {
    let provenance = InstallationProvenance::new(InstallationOrigin::ApplicationDeclaration, 2);
    let error = crate::Error::Composition(CompositionDiagnostics::new(vec![
        CompositionDiagnostic::MissingDependency {
            plugin: PluginId::new("fixture/plugin").expect("valid plugin ID"),
            provenance,
            target: RelationTarget::Slot(
                PluginSlotId::new("fixture/database").expect("valid slot ID"),
            ),
        },
    ]));
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Configure,
        error,
    ))
    .failure();

    assert_eq!(failure.diagnostics.len(), 1);
    assert_eq!(
        failure.diagnostics[0].resources,
        [
            "plugin:fixture/plugin",
            "plugin-installation:application-declaration:2",
            "plugin-slot:fixture/database",
        ]
    );
}

#[test]
fn di_failures_use_stable_component_and_underlying_type_identities() {
    let error = crate::Error::Di(upwell_di::Error::MissingDependency {
        component: String::from("Friendly Worker Name"),
        component_id: String::from("worker-component"),
        dependency: String::from("fixture::Service (qualifier `primary`)"),
        type_name: String::from("fixture::Service"),
    });
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();

    assert_eq!(
        failure.diagnostics[0].resources,
        ["component:worker-component", "type:fixture::Service"]
    );
    assert!(
        !failure.diagnostics[0]
            .resources
            .iter()
            .any(|resource| resource.contains("qualifier"))
    );
}

#[test]
fn orphan_provider_failures_emit_provider_component_and_type_identities() {
    let error = crate::Error::Di(upwell_di::Error::ProviderComponentMissing(Box::new(
        upwell_di::ProviderComponentMissing {
            trait_name: String::from("Friendly Service"),
            trait_type: String::from("fixture::Service"),
            component: String::from("Friendly Provider"),
            component_type: String::from("fixture::Provider"),
            qualifier: String::from("primary"),
        },
    )));
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-provider-component-missing");
    assert_eq!(
        diagnostic.resources,
        [
            "provider:fixture::Service:fixture::Provider:primary",
            "component:fixture::Provider",
            "type:fixture::Service",
            "type:fixture::Provider",
        ]
    );
    assert_eq!(
        diagnostic.fix.as_deref(),
        Some(
            "Register the provider's concrete component or remove the orphan provider descriptor."
        )
    );
}

#[test]
fn scope_violation_diagnostics_include_component_type_and_scope_identities() {
    let consumer_scope =
        upwell_core::ScopeId::new("fixture/request").expect("valid consumer scope identity");
    let dependency_scope =
        upwell_core::ScopeId::new("fixture/connection").expect("valid dependency scope identity");
    let error = crate::Error::Di(upwell_di::Error::ScopeViolation(Box::new(
        upwell_di::ScopeViolation {
            component: String::from("Friendly Consumer"),
            component_id: String::from("consumer-component"),
            dependency: String::from("Friendly Dependency"),
            dependency_type: String::from("fixture::Dependency"),
            component_scope: "Request",
            component_scope_id: consumer_scope,
            dependency_scope: "Connection",
            dependency_scope_id: dependency_scope,
        },
    )));
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();

    assert_eq!(
        failure.diagnostics[0].resources,
        [
            "component:consumer-component",
            "type:fixture::Dependency",
            "scope:fixture/request",
            "scope:fixture/connection",
        ]
    );
}

#[test]
fn scope_unreachable_diagnostics_include_consumer_and_candidate_provider_identities() {
    let consumer_scope =
        upwell_core::ScopeId::new("fixture/request").expect("valid consumer scope identity");
    let provider_scope =
        upwell_core::ScopeId::new("fixture/sibling").expect("valid provider scope identity");
    let error = crate::Error::Di(upwell_di::Error::ScopeUnreachableDependency(Box::new(
        upwell_di::ScopeUnreachableDependency {
            component: String::from("Friendly Consumer"),
            component_id: String::from("consumer-component"),
            dependency: String::from("Friendly Service"),
            dependency_type: String::from("fixture::Service"),
            component_scope: String::from("Request"),
            component_scope_id: consumer_scope,
            providers: vec![upwell_di::ScopeUnreachableProvider {
                component: String::from("Friendly Provider"),
                component_id: String::from("provider-component"),
                component_type: String::from("fixture::Provider"),
                scope: String::from("Sibling"),
                scope_id: provider_scope,
                qualifier: String::from("primary"),
            }],
        },
    )));
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-scope-unreachable");
    assert_eq!(
        diagnostic.resources,
        [
            "component:consumer-component",
            "type:fixture::Service",
            "scope:fixture/request",
            "provider:fixture::Service:fixture::Provider:primary",
            "component:provider-component",
            "type:fixture::Provider",
            "scope:fixture/sibling",
        ]
    );
    assert_eq!(
        diagnostic.fix.as_deref(),
        Some(
            "Move a provider into the consumer's scope chain or move the consumer beneath a provider scope."
        )
    );
}

#[test]
fn actual_prepare_failure_preserves_component_id_distinct_from_name() {
    let result = App::<DiagnosticFailureProtocol>::builder("diagnostic-failure")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&DIAGNOSTIC_CONSUMER)
        .component_descriptor(&DIAGNOSTIC_DEPENDENCY)
        .prepare();
    let error = match result {
        Ok(_) => panic!("sibling scope dependency is rejected"),
        Err(error) => error,
    };
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let resources = &failure.diagnostics[0].resources;

    assert!(resources.contains(&String::from("component:stable-consumer-id")));
    assert!(!resources.contains(&String::from("component:Friendly Consumer Name")));
    assert!(resources.contains(&format!(
        "type:{}",
        std::any::type_name::<DiagnosticDependency>()
    )));
    assert!(resources.contains(&String::from("scope:test/diagnostic-consumer")));
    assert!(resources.contains(&String::from("scope:test/diagnostic-dependency")));
}

#[test]
fn actual_prepare_failure_classifies_sibling_trait_provider_as_scope_unreachable() {
    let result = App::<DiagnosticProviderFailureProtocol>::builder("diagnostic-provider-failure")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&DIAGNOSTIC_PROVIDER_CONSUMER)
        .component_descriptor(&DIAGNOSTIC_PROVIDER_COMPONENT)
        .prepare();
    let error = match result {
        Ok(_) => panic!("sibling provider is unreachable"),
        Err(error) => error,
    };
    let failure = super::ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure();
    let diagnostic = &failure.diagnostics[0];

    assert_eq!(diagnostic.code, "upwell/tooling-scope-unreachable");
    assert!(
        diagnostic
            .resources
            .contains(&String::from("component:stable-provider-consumer-id"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("component:stable-provider-component-id"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("scope:test/diagnostic-consumer"))
    );
    assert!(
        diagnostic
            .resources
            .contains(&String::from("scope:test/diagnostic-dependency"))
    );
    assert!(diagnostic.resources.contains(&format!(
        "type:{}",
        std::any::type_name::<dyn DiagnosticProvider>()
    )));
    assert!(diagnostic.resources.iter().any(|resource| {
        resource.starts_with("provider:")
            && resource.contains(std::any::type_name::<dyn DiagnosticProvider>())
            && resource.contains(std::any::type_name::<DiagnosticProviderComponent>())
            && resource.ends_with(":sibling")
    }));
}

#[test]
fn invalid_envelope_does_not_open_or_truncate_response_path() {
    let fixture = TempFixture::new("upwell-app-invalid-before-open");
    let path = fixture.child("response.json");
    let mut envelope = ProbeEnvelope::failure(
        probe_identity(),
        upwell_tooling_schema::ProbeFailure {
            phase: None,
            diagnostics: vec![upwell_tooling_schema::Diagnostic {
                code: String::from("invalid"),
                severity: upwell_tooling_schema::DiagnosticSeverity::Error,
                message: String::from("invalid fixture"),
                ..upwell_tooling_schema::Diagnostic::default()
            }],
            resource_kinds: Default::default(),
        },
    );

    std::fs::write(&path, "preserve-me").expect("fixture response exists");
    envelope.identity.application.clear();

    assert!(super::emit_probe_envelope(&path, &envelope).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).expect("fixture response remains readable"),
        "preserve-me"
    );
}

fn probe_identity() -> DocumentIdentity {
    DocumentIdentity {
        application: String::from("tooling-tests"),
        package: Some(PackageIdentity {
            name: String::from("upwell-app"),
            version: Some(String::from(env!("CARGO_PKG_VERSION"))),
            manifest_path: Some(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"))),
        }),
        binary: Some(BinaryTargetIdentity {
            name: String::from("tooling-tests"),
        }),
        source: Some(SourceLocation {
            file: String::from(file!()),
            line: None,
            column: None,
        }),
    }
}

/// Component proving tooling projection stays before runtime construction.
struct ProjectedComponent;

/// Independent component used to verify canonical declaration ordering.
struct AlternateComponent;

/// Component carrying a config-reload hook descriptor for lifecycle projection.
struct ReloadHookComponent;

/// First provider component used to verify provider-resource ordering.
struct FirstProvider;

/// Second provider component used to verify provider-resource ordering.
struct SecondProvider;

/// Component with one exact qualified config dependency.
struct ConfigConsumer;

/// Component consuming one plugin-contributed qualified provider.
struct ProviderConsumer;

/// Plugin-owned global arguments used to verify CLI contribution projection.
#[cfg(feature = "cli")]
#[derive(clap::Args)]
struct ProjectedPluginArgs {
    /// Enables the projected plugin fixture.
    #[arg(long)]
    projected: bool,
}

/// Trait identity used by provider projection tests.
trait OrderedProvider: Send + Sync {}

/// Plugin with relations designed to exercise hostile projection endpoints.
#[derive(Default)]
struct RelationPlugin;

/// Second facet owner reusing another contributor's local facet identity.
#[derive(Default)]
struct AlternateFacetPlugin;

const RELATION_SLOT: crate::PluginSlotId =
    crate::namespaced_id!(crate::PluginSlotId, "test/absent-slot");
const RELATION_TARGET: crate::PluginId =
    crate::namespaced_id!(crate::PluginId, "test/absent-plugin");

impl Plugin for RelationPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/relation-plugin");
    const RELATIONS: &'static [PluginRelation] = &[
        PluginRelation::new(
            RelationKind::Before,
            RelationTarget::Plugin(RELATION_TARGET),
        ),
        PluginRelation::new(RelationKind::Conflicts, RelationTarget::Slot(RELATION_SLOT)),
    ];

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.tooling().facet(
            "routes",
            1,
            upwell_tooling_schema::JsonValue::String(String::from("/health")),
        );
        contributions.tooling().resource("router", "Router");
        contributions.tooling().relationship(
            ToolingRelationshipKind::Contains,
            ToolingEndpoint::Owner,
            ToolingEndpoint::Resource("router"),
        );
    }
}

impl Plugin for AlternateFacetPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/alternate-facet");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.tooling().facet(
            "routes",
            1,
            upwell_tooling_schema::JsonValue::String(String::from("/alternate")),
        );
    }
}

impl Component for ProjectedComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "projected-component";
    const NAME: &'static str = "ProjectedComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for AlternateComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "alternate-component";
    const NAME: &'static str = "AlternateComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for ReloadHookComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "reload-hook-component";
    const NAME: &'static str = "ReloadHookComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for FirstProvider {
    type Handle = Arc<Self>;

    const ID: &'static str = "first-provider";
    const NAME: &'static str = "FirstProvider";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for SecondProvider {
    type Handle = Arc<Self>;

    const ID: &'static str = "second-provider";
    const NAME: &'static str = "SecondProvider";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for ConfigConsumer {
    type Handle = Arc<Self>;

    const ID: &'static str = "config-consumer";
    const NAME: &'static str = "ConfigConsumer";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl Component for ProviderConsumer {
    type Handle = Arc<Self>;

    const ID: &'static str = "provider-consumer";
    const NAME: &'static str = "ProviderConsumer";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

impl OrderedProvider for FirstProvider {}
impl OrderedProvider for SecondProvider {}
impl OrderedProvider for ProjectedComponent {}

fn construct(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(ProjectedComponent))),
        })
    })
}

fn dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

fn snapshot_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    DEPENDENCY_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    vec![snapshot_dependency(
        if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
            "SnapshotDependencyB"
        } else {
            "SnapshotDependencyA"
        },
    )]
}

static FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "projected",
    construct,
    dependencies,
    default: true,
}];

static SNAPSHOT_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "snapshot",
    construct,
    dependencies: snapshot_dependencies,
    default: true,
}];

fn factories() -> &'static [ComponentFactoryDescriptor] {
    &FACTORIES
}

fn snapshot_factories() -> &'static [ComponentFactoryDescriptor] {
    FACTORY_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
        &[]
    } else {
        &SNAPSHOT_FACTORIES
    }
}

fn snapshot_dependency(name: &'static str) -> DependencyDescriptor {
    DependencyDescriptor {
        name,
        ty: TypeDescriptor::of::<AlternateComponent>(name),
        cardinality: Cardinality::One,
        optional: true,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }
}

fn snapshot_hook_dependencies() -> Vec<DependencyDescriptor> {
    HOOK_DEPENDENCY_CALLS.fetch_add(1, Ordering::SeqCst);

    vec![snapshot_dependency(
        if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
            "SnapshotHookDependencyB"
        } else {
            "SnapshotHookDependencyA"
        },
    )]
}

static SNAPSHOT_HOOKS: [upwell_hooks::HookDescriptor; 1] = [upwell_hooks::HookDescriptor::new(
    3,
    TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    ConfigReload::NAME,
    config_reload_type_id,
    snapshot_hook_dependencies,
    unreachable_hook_call as HookCall,
)];

fn snapshot_hooks() -> &'static [upwell_hooks::HookDescriptor] {
    HOOK_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
        &[]
    } else {
        &SNAPSHOT_HOOKS
    }
}

static COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ProjectedComponent::ID,
    name: ProjectedComponent::NAME,
    ty: TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static SNAPSHOT_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ProjectedComponent::ID,
    name: ProjectedComponent::NAME,
    ty: TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories: snapshot_factories,
    hooks: snapshot_hooks,
    generation_snapshot: None,
};

static ALTERNATE_COMPONENT: ComponentDescriptor = ComponentDescriptor::manual(
    AlternateComponent::ID,
    AlternateComponent::NAME,
    TypeDescriptor::of::<AlternateComponent>(AlternateComponent::NAME),
    &Singleton,
);

fn config_reload_type_id() -> TypeId {
    TypeId::of::<ConfigReload>()
}

fn no_hook_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

type TestHookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn std::any::Any + Send>>> + Send + 'a>>;

fn unreachable_hook_call<'a>(
    _resolver: &'a (dyn upwell_core::ResolverCtx + Send + Sync),
    _context: &'a (dyn std::any::Any + Send + Sync),
) -> TestHookFuture<'a> {
    unreachable!("tooling projection never invokes hooks")
}

static RELOAD_HOOKS: [upwell_hooks::HookDescriptor; 1] = [upwell_hooks::HookDescriptor::new(
    1,
    TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
    ConfigReload::NAME,
    config_reload_type_id,
    no_hook_dependencies,
    unreachable_hook_call as HookCall,
)];

fn reload_hooks() -> &'static [upwell_hooks::HookDescriptor] {
    &RELOAD_HOOKS
}

fn no_component_factories() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

fn exact_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(Some("duplicate"))]
}

fn ambiguous_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(None)]
}

fn missing_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(Some("missing"))]
}

fn config_dependency(qualifier: Option<&'static str>) -> DependencyDescriptor {
    DependencyDescriptor {
        name: DuplicateConfig::NAME,
        ty: TypeDescriptor::of::<DuplicateConfig>(DuplicateConfig::NAME),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier,
        config: true,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Live,
    }
}

fn construct_config_consumer(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    unreachable!("tooling projection never constructs config consumers")
}

static CONFIG_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: construct_config_consumer,
    dependencies: exact_config_dependency,
    default: true,
}];

fn config_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &CONFIG_CONSUMER_FACTORIES
}

static RELOAD_HOOK_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ReloadHookComponent::ID,
    name: ReloadHookComponent::NAME,
    ty: TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories: no_component_factories,
    hooks: reload_hooks,
    generation_snapshot: None,
};

static AMBIGUOUS_CONFIG_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::new(
        2,
        TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
        ConfigReload::NAME,
        config_reload_type_id,
        ambiguous_config_dependency,
        unreachable_hook_call as HookCall,
    )];

static MISSING_CONFIG_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::new(
        4,
        TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
        ConfigReload::NAME,
        config_reload_type_id,
        missing_config_dependency,
        unreachable_hook_call as HookCall,
    )];

fn ambiguous_config_hooks() -> &'static [upwell_hooks::HookDescriptor] {
    &AMBIGUOUS_CONFIG_HOOKS
}

fn missing_config_hooks() -> &'static [upwell_hooks::HookDescriptor] {
    &MISSING_CONFIG_HOOKS
}

static AMBIGUOUS_CONFIG_HOOK_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ReloadHookComponent::ID,
    name: ReloadHookComponent::NAME,
    ty: TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories: no_component_factories,
    hooks: ambiguous_config_hooks,
    generation_snapshot: None,
};

static MISSING_CONFIG_HOOK_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ReloadHookComponent::ID,
    name: ReloadHookComponent::NAME,
    ty: TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
    scope: &Singleton,
    condition: None,
    factories: no_component_factories,
    hooks: missing_config_hooks,
    generation_snapshot: None,
};

static CONFIG_CONSUMER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ConfigConsumer::ID,
    name: ConfigConsumer::NAME,
    ty: TypeDescriptor::of::<ConfigConsumer>(ConfigConsumer::NAME),
    scope: &Singleton,
    condition: None,
    factories: config_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

fn erase_first_provider(component: &BoxedComponent) -> BoxedComponent {
    let concrete = component
        .value
        .downcast_ref::<Arc<FirstProvider>>()
        .expect("first provider has the declared stored type");
    let value: Arc<dyn OrderedProvider> = Arc::clone(concrete) as Arc<dyn OrderedProvider>;

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
        value: Box::new(value),
    }
}

fn erase_second_provider(component: &BoxedComponent) -> BoxedComponent {
    let concrete = component
        .value
        .downcast_ref::<Arc<SecondProvider>>()
        .expect("second provider has the declared stored type");
    let value: Arc<dyn OrderedProvider> = Arc::clone(concrete) as Arc<dyn OrderedProvider>;

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
        value: Box::new(value),
    }
}

static FIRST_BEFORE_SECOND: [ProviderOrder; 1] = [ProviderOrder {
    target: TypeDescriptor::of::<SecondProvider>(SecondProvider::NAME),
    traits: &[TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider")],
    direction: ProviderOrderDirection::Before,
}];

static FIRST_PROVIDER_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
    concrete_ty: TypeDescriptor::of::<FirstProvider>(FirstProvider::NAME),
    qualifier: "first",
    primary: false,
    priority: 0,
    ordering: &FIRST_BEFORE_SECOND,
    erase: erase_first_provider,
};

static SECOND_PROVIDER_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
    concrete_ty: TypeDescriptor::of::<SecondProvider>(SecondProvider::NAME),
    qualifier: "second",
    primary: false,
    priority: 0,
    ordering: &[],
    erase: erase_second_provider,
};

fn erase_projected_provider(component: &BoxedComponent) -> BoxedComponent {
    let concrete = component
        .value
        .downcast_ref::<Arc<ProjectedComponent>>()
        .expect("projected provider has the declared stored type");
    let value: Arc<dyn OrderedProvider> = Arc::clone(concrete) as Arc<dyn OrderedProvider>;

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
        value: Box::new(value),
    }
}

static PROJECTED_PROVIDER_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
    concrete_ty: TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    qualifier: "projected",
    primary: false,
    priority: 0,
    ordering: &[],
    erase: erase_projected_provider,
};

fn provider_consumer_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "OrderedProvider",
        ty: TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: Some("projected"),
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

static PROVIDER_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "static",
        construct: construct_config_consumer,
        dependencies: provider_consumer_dependencies,
        default: true,
    }];

fn provider_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &PROVIDER_CONSUMER_FACTORIES
}

static PROVIDER_CONSUMER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ProviderConsumer::ID,
    name: ProviderConsumer::NAME,
    ty: TypeDescriptor::of::<ProviderConsumer>(ProviderConsumer::NAME),
    scope: &Singleton,
    condition: None,
    factories: provider_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};
static DISPLACED_COMPONENT: ComponentDescriptor = ComponentDescriptor::manual(
    "protocol-projected-component",
    "ProtocolProjectedComponent",
    TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    &Singleton,
);

#[derive(serde::Deserialize)]
struct DuplicateConfig {
    _enabled: bool,
}

impl ConfigProperties for DuplicateConfig {
    const NAME: &'static str = "DuplicateConfig";
}

/// Plugin contributing the first identical config binding.
#[derive(Default)]
struct FirstConfigPlugin;

/// Plugin contributing the later identical config binding.
#[derive(Default)]
struct SecondConfigPlugin;

impl Plugin for FirstConfigPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/first-config");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.config::<DuplicateConfig>(
            crate::namespaced_id!(crate::ContributionId, "test/first-config"),
            "duplicate",
        );
    }
}

impl Plugin for SecondConfigPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/second-config");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.config::<DuplicateConfig>(
            crate::namespaced_id!(crate::ContributionId, "test/second-config"),
            "duplicate",
        );
    }
}

/// Plugin whose component is displaced by later protocol registration.
#[derive(Default)]
struct DisplacedComponentPlugin;

/// Plugin contributing both a component and its trait provider.
#[derive(Default)]
struct ProviderContributionPlugin;

/// Early plugin contributing one parser-visible argument provider.
#[cfg(feature = "cli")]
#[derive(Default)]
struct CliContributionPlugin;

impl Plugin for DisplacedComponentPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/displaced-component");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/projected-component"),
            COMPONENT,
        );
    }
}

impl Plugin for ProviderContributionPlugin {
    const ID: crate::PluginId =
        crate::namespaced_id!(crate::PluginId, "test/provider-contribution");

    fn contribute(self, contributions: &mut PluginContributions) {
        contributions.component_descriptor(
            crate::namespaced_id!(crate::ContributionId, "test/provider-component"),
            COMPONENT,
        );
        contributions.provider(
            crate::namespaced_id!(crate::ContributionId, "test/provider-binding"),
            PROJECTED_PROVIDER_DESCRIPTOR,
        );
    }
}

#[cfg(feature = "cli")]
impl Plugin for CliContributionPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/cli-contribution");

    fn contribute(self, _contributions: &mut PluginContributions) {}

    fn cli(&self, cli: &mut crate::PluginCliRegistrar) {
        cli.args::<ProjectedPluginArgs>(crate::namespaced_id!(
            crate::ContributionId,
            "test/cli-args"
        ));
    }
}

/// Protocol replacing the plugin component with the final manual descriptor.
#[derive(Default)]
struct DisplacingProtocol;

impl ProtocolDefinition for DisplacingProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/displacing");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, registry: &mut AppRegistry) {
        registry.components.push(DISPLACED_COMPONENT);
    }

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

const PROJECTION_SLOT: PluginSlotId = crate::namespaced_id!(PluginSlotId, "test/projection-slot");

/// Optional protocol default used by replacement and suppression projection tests.
#[derive(Default)]
struct ProjectionDefaultPlugin;

impl Plugin for ProjectionDefaultPlugin {
    const ID: crate::PluginId = crate::namespaced_id!(crate::PluginId, "test/projection-default");

    fn contribute(self, _contributions: &mut PluginContributions) {}
}

/// Application-selected replacement used by projection tests.
#[derive(Default)]
struct ProjectionReplacementPlugin;

impl Plugin for ProjectionReplacementPlugin {
    const ID: crate::PluginId =
        crate::namespaced_id!(crate::PluginId, "test/projection-replacement");

    fn contribute(self, _contributions: &mut PluginContributions) {}
}

/// Protocol declaring one optional default plugin slot.
#[derive(Default)]
struct ProjectionPluginProtocol;

impl ProtocolDefinition for ProjectionPluginProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "test/projection-plugins");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }

    fn register_plugins(plugins: &mut ProtocolPluginRegistrar) {
        plugins.optional_default(PROJECTION_SLOT, ProjectionDefaultPlugin);
    }
}

#[test]
fn prepared_projection_is_read_only_stable_and_redacted() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    FACTORY_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    DEPENDENCY_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    HOOK_DESCRIPTOR_CALLS.store(0, Ordering::SeqCst);
    HOOK_DEPENDENCY_CALLS.store(0, Ordering::SeqCst);
    PROJECTION_CALLBACK_POISONED.store(false, Ordering::SeqCst);

    let prepared = App::<()>::builder("tooling-projection")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&SNAPSHOT_COMPONENT)
        .prepare()
        .expect("application prepares");
    let prepared_callback_counts = descriptor_callback_counts();

    PROJECTION_CALLBACK_POISONED.store(true, Ordering::SeqCst);

    let first = prepared
        .tooling_document()
        .expect("prepared state projects");
    let second = prepared
        .tooling_document()
        .expect("projection can be read repeatedly");

    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    assert!(prepared_callback_counts.iter().all(|count| *count > 0));
    assert_eq!(descriptor_callback_counts(), prepared_callback_counts);
    assert_eq!(
        first
            .to_canonical_json()
            .expect("first document serializes"),
        second
            .to_canonical_json()
            .expect("second document serializes")
    );
    assert!(first.resources.iter().any(|resource| {
        resource.id == "component:projected-component"
            && resource.labels["rust-type"].ends_with("ProjectedComponent")
            && resource.labels["construction"] == "planned-factory"
            && resource.labels["factory-selection"] == "default"
            && resource.labels["factory-candidate-count"] == "1"
            && resource.labels["factory-explicit-count"] == "0"
    }));
    assert!(first.resources.iter().any(|resource| {
        resource
            .id
            .starts_with("hook:projected-component:config_reload:")
    }));
    assert!(first.relationships.iter().any(|relationship| {
        relationship.from == "component:projected-component"
            && relationship.labels.get("name") == Some(&String::from("SnapshotDependencyA"))
    }));
    assert!(
        !first
            .to_canonical_json()
            .expect("document serializes")
            .contains("TypeId")
    );
    PROJECTION_CALLBACK_POISONED.store(false, Ordering::SeqCst);
}

fn descriptor_callback_counts() -> [usize; 4] {
    [
        FACTORY_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        DEPENDENCY_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        HOOK_DESCRIPTOR_CALLS.load(Ordering::SeqCst),
        HOOK_DEPENDENCY_CALLS.load(Ordering::SeqCst),
    ]
}

#[test]
fn displaced_plugin_contribution_points_to_the_final_registry_resource() {
    let document = App::<DisplacingProtocol>::builder("displaced-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<DisplacedComponentPlugin>()
        .prepare()
        .expect("application prepares")
        .tooling_document()
        .expect("prepared state projects");
    let contribution = document
        .resources
        .iter()
        .find(|resource| {
            resource.id == "contribution:plugin:test/displaced-component:test/projected-component"
        })
        .expect("contribution resource exists");
    assert_eq!(contribution.labels["decision"], "displaced");
    assert_eq!(
        contribution.labels["requested-target"],
        "component:projected-component"
    );
    assert_eq!(
        contribution.labels["effective-target"],
        "component:protocol-projected-component"
    );
    assert!(!contribution.labels.contains_key("applied-target"));
    assert!(!document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contributes
            && relationship.from == contribution.id
            && relationship.to == "component:protocol-projected-component"
    }));
}

#[test]
fn replacement_and_suppression_decisions_project_explicitly() {
    let replacement = App::<ProjectionPluginProtocol>::builder("replacement")
        .config_source(ConfigManager::<Toml>::empty())
        .with_plugin_declarations(|plugins| {
            plugins.replace(PROJECTION_SLOT, ProjectionReplacementPlugin);
        })
        .prepare()
        .expect("replacement prepares")
        .tooling_document()
        .expect("replacement projects");
    let suppression = App::<ProjectionPluginProtocol>::builder("suppression")
        .config_source(ConfigManager::<Toml>::empty())
        .with_plugin_declarations(|plugins| plugins.suppress(PROJECTION_SLOT))
        .prepare()
        .expect("suppression prepares")
        .tooling_document()
        .expect("suppression projects");

    assert!(replacement.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Replaces
            && relationship.from == "plugin:test/projection-replacement"
            && relationship.to == "plugin:test/projection-default"
    }));
    assert!(suppression.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Suppresses
            && relationship.to == "plugin:test/projection-default"
    }));
    assert!(suppression.resources.iter().any(|resource| {
        resource.id == "plugin:test/projection-default"
            && resource.labels["decision"] == "suppressed"
    }));
    assert!(replacement.resources.iter().any(|resource| {
        resource.id == "plugin:test/projection-replacement"
            && resource.labels["effective"] == "true"
    }));
    assert!(
        suppression
            .resources
            .iter()
            .any(|resource| { resource.id == "plugin-slot:test/projection-slot" })
    );
    assert!(suppression.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "suppression:test/projection-slot"
            && relationship.to == "plugin-slot:test/projection-slot"
            && relationship.labels["role"] == "suppressed-slot"
    }));
}

#[test]
fn selected_plugin_component_and_provider_retain_provenance_and_resolution() {
    let document = App::<()>::builder("provider-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<ProviderContributionPlugin>()
        .component_descriptor(&PROVIDER_CONSUMER_COMPONENT)
        .prepare()
        .expect("provider contribution prepares")
        .tooling_document()
        .expect("provider contribution projects");
    let component = document
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("contributed component exists");
    let provider = document
        .resources
        .iter()
        .find(|resource| {
            resource.kind == upwell_tooling_schema::ResourceKind::Provider
                && resource.labels.get("qualifier") == Some(&String::from("projected"))
        })
        .expect("contributed provider exists");

    assert_eq!(
        component
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/provider-contribution")
    );
    assert_eq!(
        provider
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/provider-contribution")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "component:provider-consumer"
            && relationship.to == provider.id
            && relationship.labels["role"] == "resolved-provider"
            && relationship.labels["selection-reason"] == "qualified"
            && relationship.labels["resolution"] == "eager"
            && relationship.labels["observation"] == "snapshot"
            && relationship.labels["cardinality"] == "one"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == "component:provider-consumer"
            && relationship.to.starts_with("type:")
            && !relationship.labels.contains_key("role")
    }));
}

#[test]
#[cfg(feature = "cli")]
fn cli_providers_project_as_owned_contribution_resources() {
    let protocol = ProtocolPluginRegistrar::new(<() as ProtocolDefinition>::ID);
    let mut application = crate::ApplicationPluginRegistrar::new();

    application.register::<CliContributionPlugin>();

    let mut catalog = crate::EarlyPluginCatalog::resolve(protocol, application)
        .expect("CLI plugin catalog resolves");
    let command = clap::Command::new("cli-contribution");
    let framework = clap::Command::new("cli-contribution");

    catalog
        .augment_cli(command, framework, &[], false)
        .expect("CLI provider composes");

    let document = App::<()>::builder("cli-contribution")
        .config_source(ConfigManager::<Toml>::empty())
        .with_early_plugin_catalog(catalog)
        .prepare()
        .expect("CLI contribution prepares")
        .tooling_document()
        .expect("CLI contribution projects");
    let id = "contribution:plugin:test/cli-contribution:test/cli-args";
    let contribution = document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .expect("CLI contribution resource exists");

    assert_eq!(
        contribution.kind,
        upwell_tooling_schema::ResourceKind::Contribution
    );
    assert_eq!(contribution.labels["provider-kind"], "args");
    assert_eq!(
        contribution
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/cli-contribution")
    );
    assert_eq!(
        contribution
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.origin.as_deref()),
        Some("cli-provider")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contributes
            && relationship.from == "plugin:test/cli-contribution"
            && relationship.to == id
    }));
}

#[test]
fn tooling_feature_projects_without_cli() {
    let prepared = App::<()>::builder("tooling-without-cli")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("CLI-independent application prepares");
    let document = prepared
        .tooling_document()
        .expect("CLI-independent state projects");

    assert_eq!(document.protocol, "upwell/none");
    assert!(document.cli.is_none());
}

#[test]
fn construction_ordinals_preserve_exact_independent_declaration_order() {
    let first = App::<()>::builder("permuted")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&COMPONENT)
        .component_descriptor(&ALTERNATE_COMPONENT)
        .prepare()
        .expect("first declaration order prepares")
        .tooling_document()
        .expect("first declaration order projects");
    let second = App::<()>::builder("permuted")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&ALTERNATE_COMPONENT)
        .component_descriptor(&COMPONENT)
        .prepare()
        .expect("second declaration order prepares")
        .tooling_document()
        .expect("second declaration order projects");

    let first_projected = first
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("projected component exists");
    let first_alternate = first
        .resources
        .iter()
        .find(|resource| resource.id == "component:alternate-component")
        .expect("alternate component exists");
    let second_projected = second
        .resources
        .iter()
        .find(|resource| resource.id == "component:projected-component")
        .expect("projected component exists");
    let second_alternate = second
        .resources
        .iter()
        .find(|resource| resource.id == "component:alternate-component")
        .expect("alternate component exists");

    assert_eq!(first_projected.labels["plan-ordinal"], "0");
    assert_eq!(first_alternate.labels["plan-ordinal"], "1");
    assert_eq!(second_alternate.labels["plan-ordinal"], "0");
    assert_eq!(second_projected.labels["plan-ordinal"], "1");
    assert!(first.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "component:projected-component"
            && relationship.to == "component:alternate-component"
            && relationship.labels["scope"] == Singleton::ID.as_str()
    }));
}

#[test]
fn plugin_relations_and_owner_qualified_facets_project_without_dropped_endpoints() {
    let document = App::<()>::builder("plugin-relations")
        .config_source(ConfigManager::<Toml>::empty())
        .register_plugin::<RelationPlugin>()
        .register_plugin::<AlternateFacetPlugin>()
        .prepare()
        .expect("application prepares")
        .tooling_document()
        .expect("prepared state projects");
    let plugin = document
        .resources
        .iter()
        .find(|resource| resource.id == "plugin:test/relation-plugin")
        .expect("plugin resource exists");

    assert!(
        plugin
            .facets
            .contains_key("plugin:test/relation-plugin/tooling/routes")
    );
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:test/alternate-facet"
            && resource
                .facets
                .contains_key("plugin:test/alternate-facet/tooling/routes")
    }));
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:test/absent-plugin"
            && resource.labels["decision"] == "relation-endpoint"
    }));
    assert!(
        document
            .resources
            .iter()
            .any(|resource| resource.id == "plugin-slot:test/absent-slot")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin:test/absent-plugin"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Conflicts
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin-slot:test/absent-slot"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Contains
            && relationship.from == "plugin:test/relation-plugin"
            && relationship.to == "plugin:test/relation-plugin/tooling/router"
    }));
}

/// Third-party protocol definition using only the public prepared tooling seam.
#[derive(Default)]
struct ThirdPartyProtocol;

/// Prepared third-party protocol metadata.
struct PreparedThirdPartyProtocol;

/// Third-party protocol relying on the public prepared tooling default.
#[derive(Default)]
struct DefaultToolingProtocol;

/// Prepared third-party protocol with no tooling override.
struct PreparedDefaultToolingProtocol;

/// Built third-party protocol runtime.
struct ThirdPartyRuntime;

/// Protocol whose prepared tooling metadata is intentionally invalid.
#[derive(Default)]
struct InvalidToolingProtocol;

/// Prepared protocol emitting duplicate owner-local tooling identities.
struct InvalidPreparedToolingProtocol;

impl ProtocolDefinition for ThirdPartyProtocol {
    type Prepared = PreparedThirdPartyProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "third-party/protocol");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(PreparedThirdPartyProtocol)
    }
}

impl ProtocolDefinition for DefaultToolingProtocol {
    type Prepared = PreparedDefaultToolingProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "third-party/default-tooling");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(PreparedDefaultToolingProtocol)
    }
}

impl ProtocolDefinition for InvalidToolingProtocol {
    type Prepared = InvalidPreparedToolingProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/invalid-tooling");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(InvalidPreparedToolingProtocol)
    }
}

impl PreparedProtocol for PreparedThirdPartyProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }

    fn tooling(&self, contributions: &mut crate::ToolingContributions) {
        TOOLING_CALLS.fetch_add(1, Ordering::SeqCst);
        contributions.display(crate::ResourceDisplay {
            label: Some(String::from("Third-party protocol")),
            summary: Some(String::from("Third-party tooling contract")),
            ..crate::ResourceDisplay::default()
        });
        contributions.resource("transport", "Third-party transport");
        contributions.resource_display(
            "transport",
            crate::ResourceDisplay {
                label: Some(String::from("Third-party transport")),
                ..crate::ResourceDisplay::default()
            },
        );
        contributions.relationship(
            ToolingRelationshipKind::Contains,
            ToolingEndpoint::Owner,
            ToolingEndpoint::Resource("transport"),
        );
    }
}

impl PreparedProtocol for PreparedDefaultToolingProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }
}

impl ProtocolRuntime for ThirdPartyRuntime {
    type Error = crate::Error;
}

impl PreparedProtocol for InvalidPreparedToolingProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }

    fn tooling(&self, contributions: &mut crate::ToolingContributions) {
        contributions.display(crate::ResourceDisplay {
            label: Some(String::from("Invalid tooling protocol")),
            ..crate::ResourceDisplay::default()
        });
        contributions.resource("duplicate", "First");
        contributions.resource("duplicate", "Second");
    }
}

#[test]
fn third_party_protocol_without_tooling_override_uses_default_display() {
    let document = App::<DefaultToolingProtocol>::builder("default-tooling-protocol")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("third-party protocol prepares with default tooling")
        .tooling_document()
        .expect("default protocol tooling projects");
    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:third-party/default-tooling")
        .expect("protocol resource is projected");

    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("Protocol")
    );
}

#[test]
fn third_party_protocol_projects_owner_scoped_generic_metadata() {
    TOOLING_CALLS.store(0, Ordering::SeqCst);

    let prepared = App::<ThirdPartyProtocol>::builder("third-party-protocol")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("third-party protocol prepares");
    let document = prepared
        .tooling_document()
        .expect("third-party metadata projects");

    prepared
        .tooling_document()
        .expect("third-party metadata reprojects");

    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:third-party/protocol")
        .expect("protocol resource is projected");

    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("Third-party protocol")
    );
    assert!(document.resources.iter().any(|resource| {
        resource.id == "protocol:third-party/protocol/tooling/transport"
            && resource
                .provenance
                .as_ref()
                .and_then(|value| value.owner.as_deref())
                == Some("protocol:third-party/protocol")
    }));
    assert_eq!(TOOLING_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_protocol_tooling_is_a_typed_prepare_error() {
    let result = App::<InvalidToolingProtocol>::builder("invalid-tooling")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare();
    let Err(error) = result else {
        panic!("invalid protocol tooling must fail during preparation");
    };

    assert!(matches!(
        error,
        crate::Error::ToolingContribution(
            crate::ToolingContributionError::DuplicateResource { .. }
        )
    ));
}

#[test]
fn duplicate_plugin_config_bindings_select_one_owner() {
    let document = App::<()>::builder("duplicate-plugin-config")
        .config_source(
            ConfigManager::<Toml>::from_str("[duplicate]\n_enabled = true")
                .expect("duplicate config fixture parses"),
        )
        .register_plugin::<FirstConfigPlugin>()
        .register_plugin::<SecondConfigPlugin>()
        .prepare()
        .expect("duplicate config contributions prepare")
        .tooling_document()
        .expect("duplicate config contributions project");
    let selected_config = document
        .resources
        .iter()
        .find(|resource| {
            resource.id.ends_with(":duplicate")
                && resource.kind == upwell_tooling_schema::ResourceKind::ConfigBinding
        })
        .expect("selected config binding exists");
    let mut decisions: Vec<_> = document
        .resources
        .iter()
        .filter(|resource| {
            resource.id.starts_with("contribution:plugin:test/")
                && resource.labels.get("contribution-kind") == Some(&String::from("config-binding"))
        })
        .map(|resource| resource.labels["decision"].as_str())
        .collect();

    decisions.sort_unstable();

    assert_eq!(decisions, ["applied", "duplicate"]);
    assert_eq!(
        selected_config
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.owner.as_deref()),
        Some("plugin:test/first-config")
    );
}

#[test]
fn lifecycle_projection_classifies_phases_and_links_config_reload_hooks() {
    let mut prepared = App::<()>::builder("lifecycle-projection")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&RELOAD_HOOK_COMPONENT)
        .prepare()
        .expect("lifecycle fixture prepares");

    prepared.retain_host_lifecycle(crate::app::HostLifecycleCapabilities::new(
        true, false, true, false, true,
    ));

    let document = prepared
        .tooling_document()
        .expect("lifecycle fixture projects");
    let prepare = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:prepare")
        .expect("prepare lifecycle exists");
    let reload = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:config_reload")
        .expect("config reload lifecycle exists");
    let setup = document
        .resources
        .iter()
        .find(|resource| resource.id == "lifecycle:setup")
        .expect("setup host phase exists");

    assert_eq!(prepare.labels["category"], "construction-phase");
    assert_eq!(reload.labels["category"], "runtime-event");
    assert_eq!(reload.labels["repeatable"], "true");
    assert_eq!(setup.labels["category"], "host-phase");
    assert_eq!(setup.labels["callback"], "true");
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Hooks && relationship.to == "lifecycle:config_reload"
    }));
    assert!(!document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::OrdersBefore
            && relationship.from == "lifecycle:startup"
            && relationship.to == "lifecycle:shutdown"
    }));
}

#[test]
fn provider_ordering_connects_matching_provider_resources() {
    let mut builder = App::<()>::builder("provider-ordering")
        .config_source(ConfigManager::<Toml>::empty())
        .with_component(FirstProvider)
        .with_component(SecondProvider);

    builder
        .registry_mut()
        .providers
        .extend([FIRST_PROVIDER_DESCRIPTOR, SECOND_PROVIDER_DESCRIPTOR]);

    let document = builder
        .prepare()
        .expect("provider ordering prepares")
        .tooling_document()
        .expect("provider ordering projects");
    let ordering = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::OrdersBefore
                && relationship.from.starts_with("provider:")
                && relationship.to.starts_with("provider:")
        })
        .expect("provider-resource ordering exists");

    assert!(ordering.from.contains("FirstProvider"));
    assert!(ordering.to.contains("SecondProvider"));
    assert!(ordering.labels["rust-trait"].contains("OrderedProvider"));
    assert!(!document.relationships.iter().any(|relationship| {
        matches!(
            relationship.kind,
            RelationshipKind::OrdersBefore | RelationshipKind::OrdersAfter
        ) && relationship.from.starts_with("component:")
            && relationship.to.starts_with("component:")
            && relationship.labels.contains_key("rust-trait")
    }));
}

#[test]
fn config_dependencies_target_exact_bindings_or_mark_type_ambiguity() {
    let document = App::<()>::builder("config-dependencies")
        .config_source(
            ConfigManager::<Toml>::from_str(
                "[duplicate]\n_enabled = true\n[alternate]\n_enabled = false",
            )
            .expect("config dependency fixture parses"),
        )
        .config::<DuplicateConfig>("duplicate")
        .config::<DuplicateConfig>("alternate")
        .component_descriptor(&CONFIG_CONSUMER_COMPONENT)
        .component_descriptor(&AMBIGUOUS_CONFIG_HOOK_COMPONENT)
        .with_component(ReloadHookComponent)
        .prepare()
        .expect("config dependency fixture prepares")
        .tooling_document()
        .expect("config dependency fixture projects");
    let exact = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship.from == "component:config-consumer"
                && relationship.to.starts_with("config-binding:")
        })
        .expect("exact component config dependency exists");
    let ambiguous = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship
                    .from
                    .starts_with("hook:reload-hook-component:config_reload:")
        })
        .expect("ambiguous hook config dependency exists");

    assert!(exact.to.starts_with("config-binding:"));
    assert!(exact.to.ends_with(":duplicate"));
    assert!(ambiguous.to.starts_with("type:"));
    assert_eq!(ambiguous.labels["binding-resolution"], "ambiguous");
    assert_eq!(ambiguous.labels["binding-cardinality"], "2");
}

#[test]
fn config_dependencies_distinguish_missing_bindings() {
    let document = App::<()>::builder("missing-config-dependency")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&MISSING_CONFIG_HOOK_COMPONENT)
        .with_component(ReloadHookComponent)
        .prepare()
        .expect("missing hook config dependency prepares")
        .tooling_document()
        .expect("missing hook config dependency projects");
    let missing = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship
                    .from
                    .starts_with("hook:reload-hook-component:config_reload:")
        })
        .expect("missing hook config dependency exists");

    assert_eq!(missing.labels["binding-resolution"], "missing");
    assert_eq!(missing.labels["binding-cardinality"], "0");
}
