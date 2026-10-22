//! Provider qualifiers and deferred candidate selection across scopes.

use super::*;

#[test]
fn validate_rejects_duplicate_provider_qualifier_in_one_scope() {
    let first = scoped!(
        "FirstQ",
        &Singleton,
        &[],
        TypeDescriptor::of::<u8>("FirstQ")
    );
    let second = scoped!(
        "SecondQ",
        &Singleton,
        &[],
        TypeDescriptor::of::<u16>("SecondQ")
    );
    let registry = ComponentRegistry {
        components: vec![first, second],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u8>("FirstQ"), "same", false),
            trait_provider(TypeDescriptor::of::<u16>("SecondQ"), "same", false),
        ],
    };

    assert!(matches!(
        registry.validate(),
        Err(Error::DuplicateProviderQualifier { .. })
    ));
}

#[test]
fn validate_allows_duplicate_provider_qualifier_across_scopes() {
    let first = scoped!(
        "FirstQ",
        &Singleton,
        &[],
        TypeDescriptor::of::<u8>("FirstQ")
    );
    let second = scoped!(
        "SecondQ",
        &Request,
        &[],
        TypeDescriptor::of::<u16>("SecondQ")
    );
    let registry = ComponentRegistry {
        components: vec![first, second],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u8>("FirstQ"), "same", false),
            trait_provider(TypeDescriptor::of::<u16>("SecondQ"), "same", false),
        ],
    };

    assert!(registry.validate().is_ok());
}

#[test]
fn validate_allows_deferred_trait_with_scoped_provider_beside_transient_primary() {
    static SINGLETON_DEFERRED_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: SHARED_TRAIT,
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];
    let consumer = scoped!(
        "DeferredTraitConsumer",
        &Singleton,
        &SINGLETON_DEFERRED_SHARED,
        TypeDescriptor::of::<u64>("DeferredTraitConsumer"),
    );
    let scoped_provider = scoped!(
        "ScopedProvider",
        &Singleton,
        &[],
        TypeDescriptor::of::<u8>("ScopedProvider"),
    );
    let transient_provider = scoped!(
        "TransientProvider",
        &Transient,
        &[],
        TypeDescriptor::of::<u16>("TransientProvider"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, scoped_provider, transient_provider],
        providers: vec![
            trait_provider(
                TypeDescriptor::of::<u16>("TransientProvider"),
                "transient",
                true,
            ),
            trait_provider(TypeDescriptor::of::<u8>("ScopedProvider"), "scoped", false),
        ],
    };

    // Hydration resolves through scope stores, which never hold transients,
    // so the scoped provider — not the transient global primary — is the
    // validated target.
    assert!(registry.validate().is_ok());
}

static SINGLETON_DEFERRED_SHARED: [DependencyDescriptor; 1] = [dependency(
    "SharedTrait",
    SHARED_TRAIT,
    Cardinality::One,
    ResolutionMode::Deferred,
)];

#[test]
fn validate_rejects_ambiguous_deferred_candidates() {
    let consumer = scoped!(
        "AmbiguousDeferredConsumer",
        &Singleton,
        &SINGLETON_DEFERRED_SHARED,
        TypeDescriptor::of::<u64>("AmbiguousDeferredConsumer"),
    );
    let first = scoped!(
        "FirstScoped",
        &Singleton,
        &[],
        TypeDescriptor::of::<u8>("FirstScoped")
    );
    let second = scoped!(
        "SecondScoped",
        &Singleton,
        &[],
        TypeDescriptor::of::<u16>("SecondScoped"),
    );
    let transient = scoped!(
        "TransientPrimary",
        &Transient,
        &[],
        TypeDescriptor::of::<u32>("TransientPrimary"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, first, second, transient],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
            trait_provider(TypeDescriptor::of::<u8>("FirstScoped"), "a", false),
            trait_provider(TypeDescriptor::of::<u16>("SecondScoped"), "b", false),
        ],
    };

    // The transient primary passes the global primary-count check, but after
    // transient filtering the scoped set is ambiguous with no parent fallback
    // left — hydration could never select deterministically.
    let Err(Error::AmbiguousProvider { candidates, .. }) = registry.validate() else {
        panic!("the deferred scoped set is ambiguous");
    };

    assert_eq!(
        candidates,
        ["SecondScoped", "FirstScoped"],
        "only the providers deferred hydration could select are reported, in provider order"
    );
}

#[test]
fn validate_selects_deferred_candidates_scope_locally() {
    static REQUEST_DEFERRED_SHARED: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: SHARED_TRAIT,
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];
    let consumer = scoped!(
        "RequestDeferredConsumer",
        &Request,
        &REQUEST_DEFERRED_SHARED,
        TypeDescriptor::of::<u64>("RequestDeferredConsumer"),
    );
    let local = scoped!(
        "LocalProvider",
        &Request,
        &[],
        TypeDescriptor::of::<u8>("LocalProvider"),
    );
    let parent = scoped!(
        "ParentProvider",
        &Singleton,
        &[],
        TypeDescriptor::of::<u16>("ParentProvider"),
    );
    let transient = scoped!(
        "TransientPrimary",
        &Transient,
        &[],
        TypeDescriptor::of::<u32>("TransientPrimary"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, local, parent, transient],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
            trait_provider(TypeDescriptor::of::<u8>("LocalProvider"), "local", false),
            trait_provider(TypeDescriptor::of::<u16>("ParentProvider"), "parent", false),
        ],
    };

    // The consumer's own scope has an unambiguous local provider, so the
    // cross-scope candidate set is not ambiguous for hydration.
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_walks_deferred_candidates_in_scope_chain_order() {
    static REQUEST_DEFERRED_CHAIN: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: SHARED_TRAIT,
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];
    let consumer = scoped!(
        "ChainConsumer",
        &Request,
        &REQUEST_DEFERRED_CHAIN,
        TypeDescriptor::of::<u64>("ChainConsumer"),
    );
    let connection = scoped!(
        "ConnectionProvider",
        &Connection,
        &[],
        TypeDescriptor::of::<u8>("ConnectionProvider"),
    );
    let singleton = scoped!(
        "SingletonProvider",
        &Singleton,
        &[],
        TypeDescriptor::of::<u16>("SingletonProvider"),
    );
    let transient = scoped!(
        "TransientPrimary",
        &Transient,
        &[],
        TypeDescriptor::of::<u32>("TransientPrimary"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, connection, singleton, transient],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
            trait_provider(
                TypeDescriptor::of::<u8>("ConnectionProvider"),
                "conn",
                false,
            ),
            trait_provider(
                TypeDescriptor::of::<u16>("SingletonProvider"),
                "root",
                false,
            ),
        ],
    };

    // The merged non-transient set is ambiguous, but runtime hydration walks
    // scope by scope and the connection scope selects its sole provider —
    // so this is a valid registry, not an ambiguous one.
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_groups_deferred_candidates_by_scope_identity_not_rank() {
    static REQUEST_DEFERRED_SIBLINGS: [DependencyDescriptor; 1] = [DependencyDescriptor {
        name: "SharedTrait",
        ty: SHARED_TRAIT,
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Deferred,
        observation: upwell_core::DependencyObservation::Snapshot,
    }];
    let consumer = scoped!(
        "SiblingConsumer",
        &Request,
        &REQUEST_DEFERRED_SIBLINGS,
        TypeDescriptor::of::<u64>("SiblingConsumer"),
    );
    let sibling_a = scoped!(
        "SiblingAProvider",
        &SiblingA,
        &[],
        TypeDescriptor::of::<u8>("SiblingAProvider"),
    );
    let sibling_b = scoped!(
        "SiblingBProvider",
        &SiblingB,
        &[],
        TypeDescriptor::of::<u16>("SiblingBProvider"),
    );
    let transient = scoped!(
        "TransientPrimary",
        &Transient,
        &[],
        TypeDescriptor::of::<u32>("TransientPrimary"),
    );
    let registry = ComponentRegistry {
        components: vec![consumer, sibling_a, sibling_b, transient],
        providers: vec![
            trait_provider(TypeDescriptor::of::<u32>("TransientPrimary"), "t", true),
            trait_provider(TypeDescriptor::of::<u8>("SiblingAProvider"), "a", false),
            trait_provider(TypeDescriptor::of::<u16>("SiblingBProvider"), "b", false),
        ],
    };

    // Both providers live in distinct scopes that share rank 3. Runtime walks
    // each container individually and the nearer scope selects its sole
    // provider, so grouping by rank alone would merge the siblings into a
    // false ambiguity. Grouping by scope identity keeps them separate.
    assert!(registry.validate().is_ok());
}
