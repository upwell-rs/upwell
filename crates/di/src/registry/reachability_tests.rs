use std::{future::Future, pin::Pin};

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, ScopeId, StaticScope, TypeDescriptor,
};

use super::*;
use crate::descriptors::{
    BoxedComponent, ComponentConstructionContext, ComponentFactoryDescriptor, ProviderDescriptor,
};

const HTTP_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/http");
const CONNECTION_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/connection");
const MESSAGE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/message");
const SIBLING_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling");

/// A standalone request branch with the same display label as [`MessageScope`].
struct HttpScope;

impl StaticScope for HttpScope {
    const ID: ScopeId = HTTP_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Request";
}

/// A connection boundary reachable by websocket messages.
struct ConnectionScope;

impl StaticScope for ConnectionScope {
    const ID: ScopeId = CONNECTION_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "Connection";
}

/// A websocket message boundary sharing a display label with [`HttpScope`].
struct MessageScope;

impl StaticScope for MessageScope {
    const ID: ScopeId = MESSAGE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Request";
}

/// A provider scope with a rank reachable under legacy validation only.
struct SiblingScope;

impl StaticScope for SiblingScope {
    const ID: ScopeId = SIBLING_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "Sibling";
}

fn fake_factory<'a>(
    _: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>> {
    Box::pin(async { unreachable!("validation does not construct components") })
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

static NO_DEPENDENCY_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fake_factory,
    dependencies: no_dependencies,
    default: false,
}];

fn no_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    &NO_DEPENDENCY_FACTORY
}

fn dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    static FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: fake_factory,
        dependencies: dependency,
        default: false,
    }];

    &FACTORY
}

fn provider_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    static FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: fake_factory,
        dependencies: required_provider_dependency,
        default: false,
    }];

    &FACTORY
}

fn dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "Dependency",
        ty: TypeDescriptor::of::<u16>("Dependency"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn required_provider_dependency() -> Vec<DependencyDescriptor> {
    vec![provider_dependency(Cardinality::One, None)]
}

fn component(
    id: &'static str,
    name: &'static str,
    ty: TypeDescriptor,
    scope: &'static dyn Scope,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name,
        ty,
        scope,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn reaches(consumer: ScopeId, dependency: ScopeId) -> bool {
    consumer == dependency
        || dependency == Singleton.id()
        || (consumer == MESSAGE_ID && dependency == CONNECTION_ID)
}

fn erase_unreachable(_: &BoxedComponent) -> BoxedComponent {
    unreachable!("dependency introspection does not erase providers")
}

fn provider(
    concrete_ty: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("SelectedTrait"),
        concrete_ty,
        qualifier,
        primary,
        priority: 0,
        ordering: &[],
        erase: erase_unreachable,
    }
}

fn provider_dependency(
    cardinality: Cardinality,
    qualifier: Option<&'static str>,
) -> DependencyDescriptor {
    DependencyDescriptor {
        name: "SelectedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SelectedTrait"),
        cardinality,
        optional: false,
        dynamic: false,
        qualifier,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }
}

#[test]
fn topology_rejects_equal_label_sibling_dependency() {
    let consumer = component(
        "http-consumer",
        "HttpConsumer",
        TypeDescriptor::of::<u8>("HttpConsumer"),
        &HttpScope,
        dependency_factory,
    );
    let dependency = component(
        "message-dependency",
        "Dependency",
        TypeDescriptor::of::<u16>("Dependency"),
        &MessageScope,
        no_dependency_factory,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, dependency],
        providers: Vec::new(),
    };

    assert!(registry.validate().is_ok());
    assert!(matches!(
        registry.validate_with_scope_reachability(reaches),
        Err(Error::ScopeViolation(_))
    ));
}

#[test]
fn topology_accepts_declared_ancestor_dependency() {
    let consumer = component(
        "message-consumer",
        "MessageConsumer",
        TypeDescriptor::of::<u8>("MessageConsumer"),
        &MessageScope,
        dependency_factory,
    );
    let dependency = component(
        "connection-dependency",
        "Dependency",
        TypeDescriptor::of::<u16>("Dependency"),
        &ConnectionScope,
        no_dependency_factory,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, dependency],
        providers: Vec::new(),
    };

    assert!(registry.validate_with_scope_reachability(reaches).is_ok());
}

#[test]
fn topology_rejects_higher_rank_unreachable_sibling() {
    let consumer = component(
        "http-consumer",
        "HttpConsumer",
        TypeDescriptor::of::<u8>("HttpConsumer"),
        &HttpScope,
        dependency_factory,
    );
    let dependency = component(
        "sibling-dependency",
        "Dependency",
        TypeDescriptor::of::<u16>("Dependency"),
        &SiblingScope,
        no_dependency_factory,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, dependency],
        providers: Vec::new(),
    };

    assert!(registry.validate().is_ok());
    assert!(matches!(
        registry.validate_with_scope_reachability(reaches),
        Err(Error::ScopeViolation(_))
    ));
}

#[test]
fn topology_reports_registered_trait_providers_in_unreachable_sibling_scopes() {
    let consumer = component(
        "http-consumer",
        "HttpConsumer",
        TypeDescriptor::of::<u8>("HttpConsumer"),
        &HttpScope,
        provider_dependency_factory,
    );
    let message_provider = component(
        "message-provider",
        "MessageProvider",
        TypeDescriptor::of::<u16>("MessageProvider"),
        &MessageScope,
        no_dependency_factory,
    );
    let sibling_provider = component(
        "sibling-provider",
        "SiblingProvider",
        TypeDescriptor::of::<u32>("SiblingProvider"),
        &SiblingScope,
        no_dependency_factory,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, message_provider, sibling_provider],
        providers: vec![
            provider(message_provider.ty, "message", false),
            provider(sibling_provider.ty, "sibling", false),
        ],
    };
    let error = registry
        .validate_with_scope_reachability(reaches)
        .expect_err("sibling providers are registered but unreachable");
    let Error::ScopeUnreachableDependency(error) = error else {
        panic!("expected scope-unreachable dependency classification");
    };

    assert_eq!(error.component_id, "http-consumer");
    assert_eq!(error.component, "HttpConsumer");
    assert_eq!(error.dependency_type, std::any::type_name::<dyn Send>());
    assert_eq!(error.component_scope_id, HTTP_ID);
    assert_eq!(error.component_scope, "Request");
    assert_eq!(error.providers.len(), 2);
    assert!(error.providers.iter().any(|candidate| {
        candidate.component_id == "message-provider"
            && candidate.component == "MessageProvider"
            && candidate.component_type == std::any::type_name::<u16>()
            && candidate.scope_id == MESSAGE_ID
            && candidate.scope == "Request"
            && candidate.qualifier == "message"
    }));
    assert!(error.providers.iter().any(|candidate| {
        candidate.component_id == "sibling-provider"
            && candidate.component == "SiblingProvider"
            && candidate.component_type == std::any::type_name::<u32>()
            && candidate.scope_id == SIBLING_ID
            && candidate.scope == "Sibling"
            && candidate.qualifier == "sibling"
    }));
}

#[test]
fn topology_keeps_truly_absent_trait_provider_as_missing_dependency() {
    let consumer = component(
        "http-consumer",
        "HttpConsumer",
        TypeDescriptor::of::<u8>("HttpConsumer"),
        &HttpScope,
        provider_dependency_factory,
    );
    let registry = ComponentRegistry {
        components: vec![consumer],
        providers: Vec::new(),
    };

    assert!(matches!(
        registry.validate_with_scope_reachability(reaches),
        Err(Error::MissingDependency { .. })
    ));
}

#[test]
fn provider_introspection_uses_runtime_scope_and_order_semantics() {
    let consumer = component(
        "message-consumer",
        "MessageConsumer",
        TypeDescriptor::of::<u8>("MessageConsumer"),
        &MessageScope,
        no_dependency_factory,
    );
    let local = component(
        "local-provider",
        "LocalProvider",
        TypeDescriptor::of::<u16>("LocalProvider"),
        &MessageScope,
        no_dependency_factory,
    );
    let ancestor = component(
        "ancestor-provider",
        "AncestorProvider",
        TypeDescriptor::of::<u32>("AncestorProvider"),
        &ConnectionScope,
        no_dependency_factory,
    );
    let root = component(
        "root-provider",
        "RootProvider",
        TypeDescriptor::of::<u64>("RootProvider"),
        &Singleton,
        no_dependency_factory,
    );
    let local_primary = component(
        "local-primary-provider",
        "LocalPrimaryProvider",
        TypeDescriptor::of::<i16>("LocalPrimaryProvider"),
        &MessageScope,
        no_dependency_factory,
    );
    let components = vec![consumer, local, local_primary, ancestor, root];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider(local.ty, "shared", false),
            provider(local_primary.ty, "local-primary", true),
            provider(ancestor.ty, "shared", false),
            provider(root.ty, "root", false),
        ],
    };

    let one = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &provider_dependency(Cardinality::One, None),
            &components,
            reaches,
        )
        .expect("single selection validates");
    let qualified = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &provider_dependency(Cardinality::One, Some("shared")),
            &components,
            reaches,
        )
        .expect("qualified selection validates");
    let collection = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &provider_dependency(Cardinality::Collection, None),
            &components,
            reaches,
        )
        .expect("collection selection validates");
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &provider_dependency(Cardinality::Keyed, None),
            &components,
            reaches,
        )
        .expect("keyed selection validates");
    let concrete = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &DependencyDescriptor {
                name: "RootProvider",
                ty: root.ty,
                cardinality: Cardinality::One,
                optional: false,
                dynamic: false,
                qualifier: None,
                config: false,
                resolution: ResolutionMode::Eager,
                observation: upwell_core::DependencyObservation::Snapshot,
            },
            &components,
            reaches,
        )
        .expect("concrete selection validates");

    assert!(matches!(
        one.as_slice(),
        [SelectedDependency {
            target: DependencyTarget::Provider(selected),
            reason: DependencySelectionReason::PrimaryProviderInWinningSet,
            scope: Some(scope),
            stage: Some(DependencySelectionStage::ScopePrecedence),
        }] if selected.concrete_ty.type_id == local_primary.ty.type_id
            && *scope == MESSAGE_ID
    ));
    assert!(matches!(
        qualified.as_slice(),
        [SelectedDependency {
            target: DependencyTarget::Provider(selected),
            reason: DependencySelectionReason::Qualified,
            scope: Some(scope),
            stage: Some(DependencySelectionStage::ScopePrecedence),
        }] if selected.concrete_ty.type_id == local.ty.type_id
            && *scope == MESSAGE_ID
    ));
    let collection_types = collection
        .iter()
        .map(|selected| match selected.target {
            DependencyTarget::Provider(provider) => provider.concrete_ty.type_id,
            DependencyTarget::Component(_) => panic!("trait collection selects providers"),
        })
        .collect::<Vec<_>>();
    let order = registry
        .provider_order(&components)
        .expect("provider order validates");
    let mut expected_collection = registry.providers.clone();

    expected_collection
        .sort_by_key(|provider| order[&provider.trait_ty.type_id][&provider.concrete_ty.type_id]);

    assert_eq!(collection.len(), 4);
    assert_eq!(
        collection_types,
        expected_collection
            .iter()
            .map(|provider| provider.concrete_ty.type_id)
            .collect::<Vec<_>>()
    );
    assert!(
        collection
            .iter()
            .all(|selected| selected.reason == DependencySelectionReason::Collection)
    );
    assert_eq!(keyed.len(), 3);
    assert!(keyed.iter().any(|selected| {
        matches!(
            selected.target,
            DependencyTarget::Provider(provider)
                if provider.concrete_ty.type_id == local.ty.type_id
        )
    }));
    assert!(keyed.iter().any(|selected| {
        matches!(
            selected.target,
            DependencyTarget::Provider(provider)
                if provider.concrete_ty.type_id == root.ty.type_id
        )
    }));
    assert!(!keyed.iter().any(|selected| {
        matches!(
            selected.target,
            DependencyTarget::Provider(provider)
                if provider.concrete_ty.type_id == ancestor.ty.type_id
        )
    }));
    assert!(matches!(
        concrete.as_slice(),
        [SelectedDependency {
            target: DependencyTarget::Component(selected),
            reason: DependencySelectionReason::DirectConcrete,
            scope: Some(scope),
            stage: Some(DependencySelectionStage::DirectConcrete),
        }] if selected.ty.type_id == root.ty.type_id
            && *scope == Singleton.id()
    ));
}
