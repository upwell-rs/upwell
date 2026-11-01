//! The types whose selection differs between two selection models.

use std::collections::HashSet;

use super::*;

fn model(
    components: &[ComponentDescriptor],
    providers: Vec<ProviderDescriptor>,
) -> ProviderSelectionModel {
    ProviderSelectionModel::new(components, providers, HashMap::new())
        .expect("the test providers name test components")
}

#[test]
fn equal_models_change_no_selection() {
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let providers = || vec![provider::<Near>(SELECTED_TRAIT, "near", false)];

    let changed = model(&[near], providers()).changed_selections(&model(&[near], providers()));

    assert!(changed.is_empty(), "got {changed:?}");
}

#[test]
fn an_added_provider_changes_its_trait_and_component_only() {
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let far = component::<Far>("far", factory).with_scope(FarScope::KEY);
    let consumer = component::<Consumer>("consumer", factory).with_scope(NearScope::KEY);
    let before = model(
        &[near, consumer],
        vec![provider::<Near>(SELECTED_TRAIT, "near", false)],
    );
    let after = model(
        &[near, far, consumer],
        vec![
            provider::<Near>(SELECTED_TRAIT, "near", false),
            provider::<Far>(SELECTED_TRAIT, "far", false),
        ],
    );

    let changed = before.changed_selections(&after);

    assert_eq!(
        changed,
        HashSet::from([SELECTED_TRAIT.type_id, TypeId::of::<Far>()])
    );
}

#[test]
fn a_changed_primary_changes_its_trait() {
    let near = component::<Near>("near", factory).with_scope(NearScope::KEY);
    let far = component::<Far>("far", factory).with_scope(FarScope::KEY);
    let providers = |primary| {
        vec![
            provider::<Near>(SELECTED_TRAIT, "near", primary),
            provider::<Far>(SELECTED_TRAIT, "far", false),
        ]
    };

    let changed = model(&[near, far], providers(false))
        .changed_selections(&model(&[near, far], providers(true)));

    assert_eq!(changed, HashSet::from([SELECTED_TRAIT.type_id]));
}
