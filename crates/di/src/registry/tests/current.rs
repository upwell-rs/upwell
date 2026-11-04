//! Invocation-context edges (`Current<T>`, `Scoped<S, T>`): exempt from the captive rule, but
//! never a way to leave a dependency unprovided.

use super::*;

static SINGLETON_CURRENT_REQUEST: [DependencyDescriptor; 1] = [dependency(
    "ReqComp",
    TypeDescriptor::of::<i64>("ReqComp"),
    Cardinality::One,
    ResolutionMode::Current,
)];

static SINGLETON_CURRENT_MISSING: [DependencyDescriptor; 1] = [dependency(
    "Unregistered",
    TypeDescriptor::of::<u128>("Unregistered"),
    Cardinality::One,
    ResolutionMode::Current,
)];

static SINGLETON_CURRENT_OPTIONAL: [DependencyDescriptor; 1] = [DependencyDescriptor {
    optional: true,
    ..SINGLETON_CURRENT_MISSING[0]
}];

/// A registry of a request component and a singleton declaring `$dependencies`.
macro_rules! registry_with {
    ($dependencies:expr) => {
        ComponentRegistry {
            components: vec![
                scoped!(
                    "RootComp",
                    Singleton::KEY,
                    &$dependencies,
                    TypeDescriptor::of::<i16>("RootComp"),
                ),
                scoped!(
                    "ReqComp",
                    Request::KEY,
                    &[],
                    TypeDescriptor::of::<i64>("ReqComp"),
                ),
            ],
            ..Default::default()
        }
    };
}

#[test]
fn a_singleton_may_read_a_request_component_from_the_invocation() {
    let registry = registry_with!(SINGLETON_CURRENT_REQUEST);

    registry
        .validate()
        .expect("an invocation-context edge retains nothing, so the captive rule allows it");
}

#[test]
fn an_invocation_context_edge_still_needs_a_provider() {
    let registry = registry_with!(SINGLETON_CURRENT_MISSING);

    assert!(matches!(
        registry.validate(),
        Err(Error::MissingDependency { ref component, .. }) if component == "RootComp"
    ));
}

#[test]
fn an_optional_invocation_context_edge_may_go_unprovided() {
    let registry = registry_with!(SINGLETON_CURRENT_OPTIONAL);

    registry
        .validate()
        .expect("an optional edge tolerates a missing provider");
}

static SINGLETON_EAGER_REQUEST: [DependencyDescriptor; 1] = [dependency(
    "ReqComp",
    TypeDescriptor::of::<i64>("ReqComp"),
    Cardinality::One,
    ResolutionMode::Eager,
)];

#[test]
fn a_captive_dependency_error_points_at_current() {
    let registry = registry_with!(SINGLETON_EAGER_REQUEST);

    let error = registry
        .validate()
        .expect_err("an eager singleton edge to a request component is captive");

    assert!(matches!(error, Error::ScopeViolation(_)));
    assert!(
        error.to_string().contains("`Current<..>`"),
        "the error names the sanctioned accessor: {error}"
    );
}
