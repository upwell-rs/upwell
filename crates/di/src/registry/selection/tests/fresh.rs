//! `Fresh` selection over factory-backed and factory-less providers.

use super::*;

#[test]
fn fresh_selection_reports_accessible_factoryless_providers() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(&NearScope);
    let near = component::<Near>("near", factory).with_scope(&NearScope);
    let far = component::<Far>("far", factory).with_scope(&FarScope);
    let manual = component::<Manual>("manual", manual).with_scope(&NearScope);
    let components = vec![consumer, near, far, manual];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Manual>(SELECTED_TRAIT, "manual", true),
            provider::<Far>(SELECTED_TRAIT, "far", false).priority(1),
            provider::<Near>(SELECTED_TRAIT, "near", false).priority(2),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Collection, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh dependency selection validates");

    assert_eq!(
        selected_types(&selected),
        [
            TypeId::of::<Manual>(),
            TypeId::of::<Far>(),
            TypeId::of::<Near>()
        ]
    );
}

#[test]
fn fresh_keyed_selection_reports_accessible_factoryless_providers() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(&NearScope);
    let near = component::<Near>("near", factory).with_scope(&NearScope);
    let manual = component::<Manual>("manual", manual).with_scope(&NearScope);
    let components = vec![consumer, near, manual];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Manual>(SELECTED_TRAIT, "manual", false),
            provider::<Near>(SELECTED_TRAIT, "near", false).priority(1),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<Manual>(), TypeId::of::<Near>()]
    );
}

#[test]
fn fresh_qualified_and_keyed_selection_match_ordered_runtime_precedence() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(&NearScope);
    let near = component::<Near>("near", factory).with_scope(&NearScope);
    let far = component::<Far>("far", factory).with_scope(&FarScope);
    let components = vec![consumer, near, far];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>(SELECTED_TRAIT, "shared", false).priority(20),
            provider::<Far>(SELECTED_TRAIT, "shared", false).priority(-10),
        ],
    };
    let qualified = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::One, ResolutionMode::Fresh, Some("shared")),
            &components,
            reaches,
        )
        .expect("fresh qualified selection validates");
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(selected_types(&qualified), [TypeId::of::<Near>()]);
    assert_eq!(qualified[0].scope, Some(NEAR_ID));
    assert_eq!(
        qualified[0].stage,
        Some(DependencySelectionStage::FreshScopePrecedence)
    );
    assert_eq!(selected_types(&keyed), [TypeId::of::<Near>()]);
    assert_eq!(keyed[0].scope, Some(NEAR_ID));
    assert_eq!(
        keyed[0].stage,
        Some(DependencySelectionStage::KeyedPrecedence)
    );
}

#[test]
fn fresh_keyed_collisions_prefer_nearer_scope_over_later_global_order() {
    let consumer = component::<Consumer>("consumer", factory).with_scope(&NearScope);
    let near = component::<Near>("near", factory).with_scope(&NearScope);
    let far = component::<Far>("far", factory).with_scope(&FarScope);
    let components = vec![consumer, near, far];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>(SELECTED_TRAIT, "shared", false).priority(-10),
            provider::<Far>(SELECTED_TRAIT, "shared", false).priority(20),
        ],
    };
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &selected_trait(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(selected_types(&keyed), [TypeId::of::<Near>()]);
    assert_eq!(keyed[0].scope, Some(NEAR_ID));
}
