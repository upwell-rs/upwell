use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, ScopeId, StaticScope, TypeDescriptor,
};

use super::*;
use crate::descriptors::{ComponentFactoryDescriptor, ProviderDescriptor};

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

fn no_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", no_dependencies)()
}

fn dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", || {
        vec![dependency(
            "Dependency",
            TypeDescriptor::of::<u16>("Dependency"),
            Cardinality::One,
            ResolutionMode::Eager,
        )]
    })()
}

fn provider_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", || vec![provider_dependency(
        Cardinality::One,
        None
    )])()
}

fn component(
    id: &'static str,
    name: &'static str,
    ty: TypeDescriptor,
    scope: &'static dyn Scope,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        name,
        ..planned(id, ty, factories).with_scope(scope)
    }
}

fn reaches(consumer: ScopeId, dependency: ScopeId) -> bool {
    consumer == dependency
        || dependency == Singleton.id()
        || (consumer == MESSAGE_ID && dependency == CONNECTION_ID)
}

fn provider(
    concrete_ty: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        primary,
        ..planned_provider(
            TypeDescriptor::of::<dyn Send>("SelectedTrait"),
            concrete_ty,
            qualifier,
        )
    }
}

fn provider_dependency(
    cardinality: Cardinality,
    qualifier: Option<&'static str>,
) -> DependencyDescriptor {
    DependencyDescriptor {
        qualifier,
        ..dependency(
            "SelectedTrait",
            TypeDescriptor::of::<dyn Send>("SelectedTrait"),
            cardinality,
            ResolutionMode::Eager,
        )
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

mod introspection;
