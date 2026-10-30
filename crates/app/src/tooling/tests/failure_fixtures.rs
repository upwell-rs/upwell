//! Scopes, components and protocols whose preparation fails with DI diagnostics.

use super::*;

pub(super) const DIAGNOSTIC_CONSUMER_SCOPE_ID: upwell_core::ScopeId =
    upwell_core::namespaced_id!(upwell_core::ScopeId, "test/diagnostic-consumer");
pub(super) const DIAGNOSTIC_DEPENDENCY_SCOPE_ID: upwell_core::ScopeId =
    upwell_core::namespaced_id!(upwell_core::ScopeId, "test/diagnostic-dependency");

pub(super) struct DiagnosticConsumerScope;

impl Scope for DiagnosticConsumerScope {
    const ID: upwell_core::ScopeId = DIAGNOSTIC_CONSUMER_SCOPE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Diagnostic Consumer";
}

pub(super) struct DiagnosticDependencyScope;

impl Scope for DiagnosticDependencyScope {
    const ID: upwell_core::ScopeId = DIAGNOSTIC_DEPENDENCY_SCOPE_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "Diagnostic Dependency";
}

pub(super) struct DiagnosticConsumer;
pub(super) struct DiagnosticDependency;

/// Trait requested by the topology-aware unreachable-provider fixture.
pub(super) trait DiagnosticProvider: Send + Sync {}

/// Consumer of a trait provider registered in an inaccessible sibling scope.
pub(super) struct DiagnosticProviderConsumer;

/// Trait provider registered in an inaccessible sibling scope.
pub(super) struct DiagnosticProviderComponent;

impl DiagnosticProvider for DiagnosticProviderComponent {}

pub(super) fn diagnostic_dependency() -> Vec<DependencyDescriptor> {
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

pub(super) fn diagnostic_provider_dependency() -> Vec<DependencyDescriptor> {
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

pub(super) static DIAGNOSTIC_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "diagnostic-consumer",
        construct,
        construct_ready: None,
        dependencies: diagnostic_dependency,
        role: ComponentFactoryRole::Explicit,
    }];

pub(super) fn diagnostic_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &DIAGNOSTIC_CONSUMER_FACTORIES
}

pub(super) static DIAGNOSTIC_PROVIDER_CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "diagnostic-provider-consumer",
        construct,
        construct_ready: None,
        dependencies: diagnostic_provider_dependency,
        role: ComponentFactoryRole::Explicit,
    }];

pub(super) fn diagnostic_provider_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &DIAGNOSTIC_PROVIDER_CONSUMER_FACTORIES
}

pub(super) static DIAGNOSTIC_CONSUMER: ComponentDescriptor = ComponentDescriptor {
    id: "stable-consumer-id",
    name: "Friendly Consumer Name",
    ty: TypeDescriptor::of::<DiagnosticConsumer>("DiagnosticConsumer"),
    scope: DiagnosticConsumerScope::KEY,
    condition: None,
    factories: diagnostic_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

pub(super) static DIAGNOSTIC_DEPENDENCY: ComponentDescriptor = ComponentDescriptor {
    id: "stable-dependency-id",
    name: "Friendly Dependency Name",
    ty: TypeDescriptor::of::<DiagnosticDependency>("DiagnosticDependency"),
    scope: DiagnosticDependencyScope::KEY,
    condition: None,
    factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

pub(super) static DIAGNOSTIC_PROVIDER_CONSUMER: ComponentDescriptor = ComponentDescriptor {
    id: "stable-provider-consumer-id",
    name: "Friendly Provider Consumer",
    ty: TypeDescriptor::of::<DiagnosticProviderConsumer>("DiagnosticProviderConsumer"),
    scope: DiagnosticConsumerScope::KEY,
    condition: None,
    factories: diagnostic_provider_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

pub(super) static DIAGNOSTIC_PROVIDER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: "stable-provider-component-id",
    name: "Friendly Provider Component",
    ty: TypeDescriptor::of::<DiagnosticProviderComponent>("DiagnosticProviderComponent"),
    scope: DiagnosticDependencyScope::KEY,
    condition: None,
    factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

pub(super) fn erase_diagnostic_provider(_: &BoxedComponent) -> BoxedComponent {
    unreachable!("provider validation never erases components")
}

pub(super) static DIAGNOSTIC_PROVIDER: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn DiagnosticProvider>("DiagnosticProvider"),
    concrete_ty: TypeDescriptor::of::<DiagnosticProviderComponent>("DiagnosticProviderComponent"),
    qualifier: "sibling",
    primary: false,
    priority: 0,
    ordering: &[],
    erase: erase_diagnostic_provider,
    live: |_| None,
};

pub(super) static DIAGNOSTIC_SCOPE_BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new::<DiagnosticConsumerScope>(&[ScopeParent::Root]),
    ScopeBoundary::new::<DiagnosticDependencyScope>(&[ScopeParent::Root]),
];

#[derive(Default)]
pub(super) struct DiagnosticFailureProtocol;

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
pub(super) struct DiagnosticProviderFailureProtocol;

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
