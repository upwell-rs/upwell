//! Components, providers and hooks the projection tests register.

use super::*;
use crate::test_support::uncalled_hook;

pub(super) struct ProjectedComponent;

/// Independent component used to verify canonical declaration ordering.
pub(super) struct AlternateComponent;

/// Component carrying a config-reload hook descriptor for lifecycle projection.
pub(super) struct ReloadHookComponent;

/// First provider component used to verify provider-resource ordering.
pub(super) struct FirstProvider;

/// Second provider component used to verify provider-resource ordering.
pub(super) struct SecondProvider;

/// Component with one exact qualified config dependency.
pub(super) struct ConfigConsumer;

/// Component consuming one plugin-contributed qualified provider.
pub(super) struct ProviderConsumer;

/// Plugin-owned global arguments used to verify CLI contribution projection.
#[cfg(feature = "cli")]
#[derive(clap::Args)]
pub(super) struct ProjectedPluginArgs {
    /// Enables the projected plugin fixture.
    #[arg(long)]
    projected: bool,
}

/// Trait identity used by provider projection tests.
pub(super) trait OrderedProvider: Send + Sync {}

/// Plugin with relations designed to exercise hostile projection endpoints.
#[derive(Default)]
pub(super) struct RelationPlugin;

/// Second facet owner reusing another contributor's local facet identity.
#[derive(Default)]
pub(super) struct AlternateFacetPlugin;

pub(super) const RELATION_SLOT: crate::PluginSlotId =
    crate::namespaced_id!(crate::PluginSlotId, "test/absent-slot");
pub(super) const RELATION_TARGET: crate::PluginId =
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

test_component!(ProjectedComponent, "projected-component", provides dyn OrderedProvider);
test_component!(AlternateComponent, "alternate-component");
test_component!(ReloadHookComponent, "reload-hook-component");
test_component!(FirstProvider, "first-provider", provides dyn OrderedProvider);
test_component!(SecondProvider, "second-provider", provides dyn OrderedProvider);
test_component!(ConfigConsumer, "config-consumer");
test_component!(ProviderConsumer, "provider-consumer");

impl OrderedProvider for FirstProvider {}

impl OrderedProvider for SecondProvider {}

impl OrderedProvider for ProjectedComponent {}

pub(super) fn construct(
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

pub(super) fn dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

pub(super) fn snapshot_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    DEPENDENCY_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    vec![snapshot_dependency(
        if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
            "SnapshotDependencyB"
        } else {
            "SnapshotDependencyA"
        },
    )]
}

pub(super) static FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "projected",
    construct,
    dependencies,
    role: ComponentFactoryRole::Default,
}];

pub(super) static SNAPSHOT_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "snapshot",
        construct,
        dependencies: snapshot_dependencies,
        role: ComponentFactoryRole::Default,
    }];

pub(super) fn factories() -> &'static [ComponentFactoryDescriptor] {
    &FACTORIES
}

pub(super) fn snapshot_factories() -> &'static [ComponentFactoryDescriptor] {
    FACTORY_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
        &[]
    } else {
        &SNAPSHOT_FACTORIES
    }
}

pub(super) fn snapshot_dependency(name: &'static str) -> DependencyDescriptor {
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

pub(super) fn snapshot_hook_dependencies() -> Vec<DependencyDescriptor> {
    HOOK_DEPENDENCY_CALLS.fetch_add(1, Ordering::SeqCst);

    vec![snapshot_dependency(
        if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
            "SnapshotHookDependencyB"
        } else {
            "SnapshotHookDependencyA"
        },
    )]
}

pub(super) static SNAPSHOT_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::of::<ConfigReload>(
        3,
        TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
        uncalled_hook,
    )
    .with_dependencies(snapshot_hook_dependencies)];

pub(super) fn snapshot_hooks() -> &'static [upwell_hooks::HookDescriptor] {
    HOOK_DESCRIPTOR_CALLS.fetch_add(1, Ordering::SeqCst);

    if PROJECTION_CALLBACK_POISONED.load(Ordering::SeqCst) {
        &[]
    } else {
        &SNAPSHOT_HOOKS
    }
}

pub(super) static COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<ProjectedComponent>()
    .with_factories(factories)
    .without_retention();

pub(super) static SNAPSHOT_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ProjectedComponent>()
        .with_factories(snapshot_factories)
        .with_hooks(snapshot_hooks)
        .without_retention();

pub(super) static ALTERNATE_COMPONENT: ComponentDescriptor = ComponentDescriptor::manual(
    AlternateComponent::ID,
    AlternateComponent::NAME,
    TypeDescriptor::of::<AlternateComponent>(AlternateComponent::NAME),
    Singleton::KEY,
);

pub(super) static RELOAD_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::of::<ConfigReload>(
        1,
        TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
        uncalled_hook,
    )];

pub(super) fn exact_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(Some("duplicate"))]
}

pub(super) fn ambiguous_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(None)]
}

pub(super) fn missing_config_dependency() -> Vec<DependencyDescriptor> {
    vec![config_dependency(Some("missing"))]
}

pub(super) fn config_dependency(qualifier: Option<&'static str>) -> DependencyDescriptor {
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

pub(super) fn construct_config_consumer(
    _context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    unreachable!("tooling projection never constructs config consumers")
}

pub(super) static CONFIG_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "static",
        construct: construct_config_consumer,
        dependencies: exact_config_dependency,
        role: ComponentFactoryRole::Default,
    }];

pub(super) fn config_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &CONFIG_CONSUMER_FACTORIES
}

pub(super) static RELOAD_HOOK_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ReloadHookComponent>()
        .with_hooks(|| &RELOAD_HOOKS)
        .without_retention();

pub(super) static AMBIGUOUS_CONFIG_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::of::<ConfigReload>(
        2,
        TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
        uncalled_hook,
    )
    .with_dependencies(ambiguous_config_dependency)];

pub(super) static MISSING_CONFIG_HOOKS: [upwell_hooks::HookDescriptor; 1] =
    [upwell_hooks::HookDescriptor::of::<ConfigReload>(
        4,
        TypeDescriptor::of::<ReloadHookComponent>(ReloadHookComponent::NAME),
        uncalled_hook,
    )
    .with_dependencies(missing_config_dependency)];

pub(super) static AMBIGUOUS_CONFIG_HOOK_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ReloadHookComponent>()
        .with_hooks(|| &AMBIGUOUS_CONFIG_HOOKS)
        .without_retention();

pub(super) static MISSING_CONFIG_HOOK_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ReloadHookComponent>()
        .with_hooks(|| &MISSING_CONFIG_HOOKS)
        .without_retention();

pub(super) static CONFIG_CONSUMER_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ConfigConsumer>()
        .with_factories(config_consumer_factories)
        .without_retention();

pub(super) static FIRST_BEFORE_SECOND: [ProviderOrder; 1] = [ProviderOrder {
    target: TypeDescriptor::of::<SecondProvider>(SecondProvider::NAME),
    traits: &[TypeDescriptor::of::<dyn OrderedProvider>("OrderedProvider")],
    direction: ProviderOrderDirection::Before,
}];

/// The `OrderedProvider` mapping of `T` under `qualifier`. Its erase panics, so a
/// projection regression that erases a provider fails these tests.
const fn ordered_provider<T>(qualifier: &'static str) -> ProviderDescriptor
where
    T: Component + upwell_di::ProviderOf<dyn OrderedProvider>,
    T::Handle: upwell_di::Injectable<Stored = upwell_di::Live<T>>,
{
    ProviderDescriptor {
        erase: |_| panic!("tooling projection never erases providers"),
        live: |_| None,
        ..ProviderDescriptor::of::<T, dyn OrderedProvider>("OrderedProvider").qualifier(qualifier)
    }
}

pub(super) static FIRST_PROVIDER_DESCRIPTOR: ProviderDescriptor =
    ordered_provider::<FirstProvider>("first").ordering(&FIRST_BEFORE_SECOND);

pub(super) static SECOND_PROVIDER_DESCRIPTOR: ProviderDescriptor =
    ordered_provider::<SecondProvider>("second");

pub(super) static PROJECTED_PROVIDER_DESCRIPTOR: ProviderDescriptor =
    ordered_provider::<ProjectedComponent>("projected");

pub(super) fn provider_consumer_dependencies() -> Vec<DependencyDescriptor> {
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

pub(super) static PROVIDER_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "static",
        construct: construct_config_consumer,
        dependencies: provider_consumer_dependencies,
        role: ComponentFactoryRole::Default,
    }];

pub(super) fn provider_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &PROVIDER_CONSUMER_FACTORIES
}

pub(super) static PROVIDER_CONSUMER_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ProviderConsumer>()
        .with_factories(provider_consumer_factories)
        .without_retention();
pub(super) static DISPLACED_COMPONENT: ComponentDescriptor = ComponentDescriptor::manual(
    "protocol-projected-component",
    "ProtocolProjectedComponent",
    TypeDescriptor::of::<ProjectedComponent>(ProjectedComponent::NAME),
    Singleton::KEY,
);
