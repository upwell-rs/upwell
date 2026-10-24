//! Provider introspection under runtime scope reachability and ordering.

use super::*;

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
