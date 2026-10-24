//! Fresh and deferred dependency scope checks against their targets.

use super::*;

// --- Scope validation --------------------------------------------------
//
// Stand-in types per scope: i16 = singleton, i32 = connection, i64 = request.

static SINGLETON_FRESH_REQUEST: [DependencyDescriptor; 1] = [dependency(
    "RequestFresh",
    TypeDescriptor::of::<u128>("RequestFresh"),
    Cardinality::One,
    ResolutionMode::Fresh,
)];

static SINGLETON_FRESH_SHARED_COLLECTION: [DependencyDescriptor; 1] = [dependency(
    "SharedTrait",
    SHARED_TRAIT,
    Cardinality::Collection,
    ResolutionMode::Fresh,
)];

static SINGLETON_FRESH_SHARED_KEYED: [DependencyDescriptor; 1] = [dependency(
    "SharedTrait",
    SHARED_TRAIT,
    Cardinality::Keyed,
    ResolutionMode::Fresh,
)];

static REQUEST_FRESH_CONNECTION_TARGET: [DependencyDescriptor; 1] = [dependency(
    "ConnectionFreshTarget",
    TypeDescriptor::of::<i128>("ConnectionFreshTarget"),
    Cardinality::One,
    ResolutionMode::Fresh,
)];

static CONNECTION_TARGET_DEP_ON_SHARED: [DependencyDescriptor; 1] = [dependency(
    "SharedTrait",
    SHARED_TRAIT,
    Cardinality::One,
    ResolutionMode::Eager,
)];

#[test]
fn validate_rejects_fresh_target_with_shorter_lived_scope() {
    let request = scoped!(
        "RequestFresh",
        &Request,
        &[],
        TypeDescriptor::of::<u128>("RequestFresh"),
    );
    let singleton = scoped!(
        "FreshConsumer",
        &Singleton,
        &SINGLETON_FRESH_REQUEST,
        TypeDescriptor::of::<i8>("FreshConsumer"),
    );
    let registry = ComponentRegistry {
        components: vec![singleton, request],
        ..Default::default()
    };

    assert!(matches!(
        registry.validate(),
        Err(Error::InvalidFreshDependency(_))
    ));
}

#[test]
fn validate_selects_nested_fresh_dependencies_from_target_scope() {
    let provider = scoped!(
        "ConnectionProvider",
        &Connection,
        &[],
        TypeDescriptor::of::<usize>("ConnectionProvider"),
    );
    let target = scoped!(
        "ConnectionFreshTarget",
        &Connection,
        &CONNECTION_TARGET_DEP_ON_SHARED,
        TypeDescriptor::of::<i128>("ConnectionFreshTarget"),
    );
    let consumer = scoped!(
        "RequestFreshConsumer",
        &Request,
        &REQUEST_FRESH_CONNECTION_TARGET,
        TypeDescriptor::of::<isize>("RequestFreshConsumer"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, target, provider],
        providers: vec![trait_provider(provider.ty, "connection", false)],
    };
    let selection = registry
        .provider_selection_model(&registry.components)
        .expect("provider selection model validates");
    let selected_from_target_scope = Cell::new(false);

    registry
        .validate_fresh_dependencies_with(
            &registry.components,
            &selection,
            &|consumer_scope, dependency_scope| {
                if consumer_scope.id() == Connection.id()
                    && dependency_scope.id() == Connection.id()
                {
                    selected_from_target_scope.set(true);
                }

                scope_allows_by_rank(consumer_scope, dependency_scope)
            },
        )
        .expect("nested fresh dependency validates");

    assert!(
        selected_from_target_scope.get(),
        "nested dependencies must be selected from the fresh target's scope"
    );
}

#[test]
fn validate_rejects_fresh_collection_with_accessible_factoryless_provider() {
    let consumer = scoped!(
        "FreshCollectionConsumer",
        &Singleton,
        &SINGLETON_FRESH_SHARED_COLLECTION,
        TypeDescriptor::of::<u64>("FreshCollectionConsumer"),
    );
    let manual = ComponentDescriptor::manual(
        "manual-provider",
        "ManualProvider",
        TypeDescriptor::of::<u8>("ManualProvider"),
        &Singleton,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, manual],
        providers: vec![trait_provider(manual.ty, "manual", false)],
    };

    assert!(matches!(
        registry.validate(),
        Err(Error::UnsupportedFreshFactory {
            component_id: Some(component_id),
            ..
        }) if component_id == "manual-provider"
    ));
}

#[test]
fn validate_rejects_fresh_keyed_with_accessible_factoryless_provider() {
    let consumer = scoped!(
        "FreshKeyedConsumer",
        &Singleton,
        &SINGLETON_FRESH_SHARED_KEYED,
        TypeDescriptor::of::<u64>("FreshKeyedConsumer"),
    );
    let manual = ComponentDescriptor::manual(
        "manual-provider",
        "ManualProvider",
        TypeDescriptor::of::<u8>("ManualProvider"),
        &Singleton,
    );
    let registry = ComponentRegistry {
        components: vec![consumer, manual],
        providers: vec![trait_provider(manual.ty, "manual", false)],
    };

    assert!(matches!(
        registry.validate(),
        Err(Error::UnsupportedFreshFactory {
            component_id: Some(component_id),
            ..
        }) if component_id == "manual-provider"
    ));
}

#[test]
fn validate_allows_lazy_dependency_on_transient_target() {
    static SINGLETON_LAZY_TRANSIENT: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "TransientLazy",
        ty: TypeDescriptor::of::<u32>("TransientLazy"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Lazy,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];
    let transient = scoped!(
        "TransientLazy",
        &Transient,
        &[],
        TypeDescriptor::of::<u32>("TransientLazy"),
    );
    let singleton = scoped!(
        "LazyConsumer",
        &Singleton,
        &SINGLETON_LAZY_TRANSIENT,
        TypeDescriptor::of::<u8>("LazyConsumer"),
    );
    let registry = ComponentRegistry {
        components: vec![singleton, transient],
        ..Default::default()
    };

    assert!(registry.validate().is_ok());
}
