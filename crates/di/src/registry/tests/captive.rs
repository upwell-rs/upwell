//! Captive-dependency rules between singleton, connection, request and transient scopes.

use super::*;

static SINGLETON_DEP_ON_REQUEST: [DependencyDescriptor; 1] = [dependency(
    "ReqComp",
    TypeDescriptor::of::<i64>("ReqComp"),
    Cardinality::One,
    ResolutionMode::Eager,
)];

static REQUEST_DEP_ON_CONNECTION: [DependencyDescriptor; 1] = [dependency(
    "ConnComp",
    TypeDescriptor::of::<i32>("ConnComp"),
    Cardinality::One,
    ResolutionMode::Eager,
)];

static SINGLETON_DEFERRED_TRANSIENT: [DependencyDescriptor; 1] = [dependency(
    "TransientDeferred",
    TypeDescriptor::of::<u32>("TransientDeferred"),
    Cardinality::One,
    ResolutionMode::Deferred,
)];

#[test]
fn validate_rejects_singleton_depending_on_request() {
    let request = scoped!(
        "ReqComp",
        Request::KEY,
        &[],
        TypeDescriptor::of::<i64>("ReqComp"),
    );
    let singleton = scoped!(
        "RootComp",
        Singleton::KEY,
        &SINGLETON_DEP_ON_REQUEST,
        TypeDescriptor::of::<i16>("RootComp"),
    );

    let registry = ComponentRegistry {
        components: vec![singleton, request],
        ..Default::default()
    };

    assert!(matches!(registry.validate(), Err(Error::ScopeViolation(_))));
}

#[test]
fn validate_allows_request_depending_on_connection() {
    let connection = scoped!(
        "ConnComp",
        Connection::KEY,
        &[],
        TypeDescriptor::of::<i32>("ConnComp"),
    );
    let request = scoped!(
        "ReqComp",
        Request::KEY,
        &REQUEST_DEP_ON_CONNECTION,
        TypeDescriptor::of::<i64>("ReqComp"),
    );

    let registry = ComponentRegistry {
        components: vec![connection, request],
        ..Default::default()
    };

    assert!(registry.validate_scopes(&registry.components).is_ok());
}

#[test]
fn validate_rejects_transient_depending_on_connection() {
    let connection = scoped!(
        "ConnComp",
        Connection::KEY,
        &[],
        TypeDescriptor::of::<i32>("ConnComp"),
    );
    let transient = scoped!(
        "TransComp",
        Transient::KEY,
        &REQUEST_DEP_ON_CONNECTION,
        TypeDescriptor::of::<i64>("TransComp"),
    );

    let registry = ComponentRegistry {
        components: vec![connection, transient],
        ..Default::default()
    };

    assert!(matches!(
        registry.validate_scopes(&registry.components),
        Err(Error::ScopeViolation(_))
    ));
}

#[test]
fn validate_rejects_deferred_transient_target() {
    let transient = scoped!(
        "TransientDeferred",
        Transient::KEY,
        &[],
        TypeDescriptor::of::<u32>("TransientDeferred"),
    );
    let singleton = scoped!(
        "DeferredConsumer",
        Singleton::KEY,
        &SINGLETON_DEFERRED_TRANSIENT,
        TypeDescriptor::of::<u64>("DeferredConsumer"),
    );
    let registry = ComponentRegistry {
        components: vec![singleton, transient],
        ..Default::default()
    };

    assert!(matches!(
        registry.validate(),
        Err(Error::DeferredTransientDependency(_))
    ));
}
