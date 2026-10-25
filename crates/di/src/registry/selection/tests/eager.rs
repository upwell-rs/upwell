//! Eager, deferred, transient-fallback and collection selection.

use super::*;

#[test]
fn eager_single_matches_global_transient_then_built_then_transient_fallback() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(NearScope::KEY);
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let transient = component::<TransientProvider>("transient", factory).with_scope(Transient::KEY);
    let components = vec![consumer, near, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>(SELECTED_TRAIT, "transient", false),
            provider::<Near>(SELECTED_TRAIT, "near", true),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("dependency selection validates");

    assert_eq!(selected_types(&selected), [TypeId::of::<Near>()]);
    assert_eq!(
        selected[0].reason,
        DependencySelectionReason::SoleProviderInWinningSet
    );
    assert_eq!(selected[0].scope, Some(NEAR_ID));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::ScopePrecedence)
    );

    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>(SELECTED_TRAIT, "transient", true),
            provider::<Near>(SELECTED_TRAIT, "near", false),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("dependency selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<TransientProvider>()]
    );
    assert_eq!(
        selected[0].reason,
        DependencySelectionReason::PrimaryProviderInWinningSet
    );
    assert_eq!(selected[0].scope, Some(Transient::ID));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::TransientPrecedence)
    );
}

#[test]
fn qualified_and_deferred_selection_preserve_runtime_transient_semantics() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(NearScope::KEY);
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let transient = component::<TransientProvider>("transient", factory).with_scope(Transient::KEY);
    let components = vec![consumer, near, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>(SELECTED_TRAIT, "shared", false).priority(-10),
            provider::<Near>(SELECTED_TRAIT, "shared", false),
        ],
    };

    let eager = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Eager, Some("shared")),
            &components,
            reaches,
        )
        .expect("eager dependency selection validates");
    let deferred = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Deferred, Some("shared")),
            &components,
            reaches,
        )
        .expect("deferred dependency selection validates");

    assert_eq!(selected_types(&eager), [TypeId::of::<TransientProvider>()]);
    assert_eq!(selected_types(&deferred), [TypeId::of::<Near>()]);
    assert_eq!(deferred[0].reason, DependencySelectionReason::Qualified);
    assert_eq!(deferred[0].scope, Some(NEAR_ID));
    assert_eq!(
        deferred[0].stage,
        Some(DependencySelectionStage::ScopePrecedence)
    );
}

#[test]
fn transient_fallback_uses_the_same_ordered_qualified_sets_as_runtime() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(Transient::KEY);
    let inaccessible = component::<Near>("inaccessible", factory).with_scope(NearScope::KEY);
    let transient = component::<TransientProvider>("transient", factory).with_scope(Transient::KEY);
    let components = vec![consumer, inaccessible, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>(SELECTED_TRAIT, "shared", false).priority(20),
            provider::<Near>(SELECTED_TRAIT, "shared", false).priority(-10),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Eager, Some("shared")),
            &components,
            reaches,
        )
        .expect("transient fallback selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<TransientProvider>()]
    );
    assert_eq!(selected[0].scope, Some(Transient::ID));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::TransientFallback)
    );
}

#[test]
fn collections_use_final_provider_order_and_keyed_collisions_use_runtime_precedence() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(NearScope::KEY);
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let far = component::<Far>("far", factory).with_scope(FarScope::KEY);
    let transient = component::<TransientProvider>("transient", factory).with_scope(Transient::KEY);
    let components = vec![consumer, near, far, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>(SELECTED_TRAIT, "shared", false).priority(20),
            provider::<TransientProvider>(SELECTED_TRAIT, "shared", false).priority(10),
            provider::<Far>(SELECTED_TRAIT, "shared", false),
        ],
    };

    let collection = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Collection, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("collection selection validates");
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Keyed, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("keyed selection validates");
    let deferred_keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Keyed, ResolutionMode::Deferred, None),
            &components,
            reaches,
        )
        .expect("deferred keyed selection validates");

    assert_eq!(
        selected_types(&collection),
        [
            TypeId::of::<Far>(),
            TypeId::of::<TransientProvider>(),
            TypeId::of::<Near>(),
        ]
    );
    assert_eq!(selected_types(&keyed), [TypeId::of::<TransientProvider>()]);
    assert_eq!(selected_types(&deferred_keyed), [TypeId::of::<Near>()]);
}

#[test]
fn direct_selection_model_construction_rejects_orphan_provider() {
    let orphan = provider::<Near>(SELECTED_TRAIT, "orphan", false);
    let error = match ProviderSelectionModel::new(&[], vec![orphan], HashMap::new()) {
        Ok(_) => panic!("orphan provider must be rejected"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::ProviderComponentMissing(_)));
}
