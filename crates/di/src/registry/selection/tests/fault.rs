//! Per-edge faults for consumers that are not components.

use super::*;

/// A second near-scoped provider that ties with [`Near`].
struct Twin;

fn concrete<T: 'static>(name: &'static str) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(name),
        Cardinality::One,
        ResolutionMode::Eager,
    )
}

fn fault(
    model: &ProviderSelectionModel,
    dependency: &DependencyDescriptor,
    consumer: upwell_core::ScopeKey,
    can_reach: fn(ScopeId, ScopeId) -> bool,
) -> Option<DependencyFault> {
    model
        .dependency_fault(dependency, consumer, can_reach)
        .expect("the edge has a per-edge outcome")
}

fn model(providers: Vec<ProviderDescriptor>) -> ProviderSelectionModel {
    let components = vec![
        component::<Near>("near", factory).with_scope(NearScope::KEY),
        component::<Twin>("twin", factory).with_scope(NearScope::KEY),
        component::<Far>("far", factory).with_scope(FarScope::KEY),
        component::<TransientProvider>("transient", factory).with_scope(Transient::KEY),
    ];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers,
    };

    registry
        .provider_selection_model(&components)
        .expect("selection model builds")
}

#[test]
fn concrete_targets_resolve_only_from_scopes_that_reach_them() {
    let model = model(Vec::new());

    let near_from_near = fault(&model, &concrete::<Near>("near"), NearScope::KEY, reaches);
    let far_from_near = fault(&model, &concrete::<Far>("far"), NearScope::KEY, reaches);
    let near_from_far = fault(&model, &concrete::<Near>("near"), FarScope::KEY, reaches);

    assert_eq!(near_from_near, None);
    assert_eq!(far_from_near, None);
    assert_eq!(
        near_from_far,
        Some(DependencyFault::Unreachable {
            scopes: vec![NEAR_ID]
        })
    );
}

#[test]
fn unprovided_traits_fail_only_required_single_and_qualified_edges() {
    let model = model(vec![provider::<Near>(SELECTED_TRAIT, "near", false)]);
    let optional = DependencyDescriptor {
        optional: true,
        ..selected_trait(Cardinality::One, ResolutionMode::Eager, Some("absent"))
    };

    let missing = fault(
        &model,
        &selected_trait(Cardinality::One, ResolutionMode::Eager, Some("absent")),
        NearScope::KEY,
        reaches,
    );
    let collection = fault(
        &model,
        &selected_trait(Cardinality::Collection, ResolutionMode::Eager, None),
        FarScope::KEY,
        reaches,
    );

    assert_eq!(missing, Some(DependencyFault::Missing));
    assert_eq!(
        fault(&model, &optional, NearScope::KEY, reaches),
        Some(DependencyFault::Missing)
    );
    assert_eq!(collection, None);
}

#[test]
fn providers_outside_the_reachable_chain_are_unreachable_unless_optional() {
    let model = model(vec![provider::<Near>(SELECTED_TRAIT, "near", false)]);
    let required = selected_trait(Cardinality::One, ResolutionMode::Eager, None);
    let optional = DependencyDescriptor {
        optional: true,
        ..required
    };

    assert_eq!(
        fault(&model, &required, FarScope::KEY, reaches),
        Some(DependencyFault::Unreachable {
            scopes: vec![NEAR_ID]
        })
    );
    assert_eq!(fault(&model, &optional, FarScope::KEY, reaches), None);
    assert_eq!(fault(&model, &required, NearScope::KEY, reaches), None);
}

#[test]
fn tied_single_providers_are_ambiguous_and_config_edges_are_exempt() {
    let model = model(vec![
        provider::<Near>(SELECTED_TRAIT, "near", false),
        provider::<Twin>(SELECTED_TRAIT, "twin", false),
    ]);
    let config = DependencyDescriptor {
        config: true,
        ..concrete::<Manual>("manual")
    };

    let tied = fault(
        &model,
        &selected_trait(Cardinality::One, ResolutionMode::Eager, None),
        NearScope::KEY,
        reaches,
    );

    assert_eq!(
        tied,
        Some(DependencyFault::Ambiguous {
            candidates: vec![String::from("near"), String::from("twin")]
        })
    );
    assert_eq!(fault(&model, &config, NearScope::KEY, reaches), None);
}

#[test]
fn deferred_edges_reject_transient_targets_of_every_cardinality() {
    let model = model(vec![provider::<TransientProvider>(
        SELECTED_TRAIT,
        "transient",
        false,
    )]);
    let concrete = DependencyDescriptor {
        resolution: ResolutionMode::Deferred,
        ..concrete::<TransientProvider>("transient")
    };

    let collection = fault(
        &model,
        &selected_trait(Cardinality::Collection, ResolutionMode::Deferred, None),
        NearScope::KEY,
        reaches,
    );

    assert_eq!(
        fault(&model, &concrete, NearScope::KEY, reaches),
        Some(DependencyFault::DeferredTransient)
    );
    assert_eq!(collection, Some(DependencyFault::DeferredTransient));
}

#[test]
fn fresh_targets_must_live_in_a_reachable_scope() {
    let model = model(Vec::new());
    let fresh = DependencyDescriptor {
        resolution: ResolutionMode::Fresh,
        ..concrete::<Near>("near")
    };

    assert_eq!(
        fault(&model, &fresh, FarScope::KEY, reaches),
        Some(DependencyFault::FreshUnreachable { scope: NEAR_ID })
    );
    assert_eq!(fault(&model, &fresh, NearScope::KEY, reaches), None);
}
