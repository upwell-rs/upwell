//! Planning the provider a scope build selects across scopes and transients.

use super::*;

#[test]
fn duplicate_qualifiers_across_scopes_plan_the_runtime_visible_provider() {
    let parent = descriptor::<ParentSharedProvider>(
        "parent-shared",
        "ParentSharedProvider",
        &PARENT,
        empty_factory,
    );
    let child = descriptor::<ChildSharedProvider>(
        "child-shared",
        "ChildSharedProvider",
        &CHILD,
        empty_factory,
    );
    let consumer = descriptor::<ChildSharedConsumer>(
        "child-consumer",
        "ChildSharedConsumer",
        &CHILD,
        shared_provider_factory,
    );
    let descriptors = [parent, consumer, child];
    let selection = selection_with(
        &descriptors,
        vec![
            provider::<ParentSharedProvider, dyn SharedProvider>("shared", 0, &[]),
            provider::<ChildSharedProvider, dyn SharedProvider>("shared", 0, &[]),
        ],
    );
    let selected = selection.selected_dependencies_with_scope_reachability(
        &consumer,
        &shared_provider_dependency(),
        |consumer, dependency| topology().is_reachable(&consumer, &dependency),
    );
    let plan = ScopePlan::partition(&descriptors, &selection, &topology())
        .expect("cross-scope duplicate qualifier plans");

    assert!(matches!(
        selected.as_slice(),
        [upwell_di::SelectedDependency {
            target: DependencyTarget::Provider(provider),
            ..
        }] if provider.concrete_ty.type_id == TypeId::of::<ChildSharedProvider>()
    ));
    assert_eq!(
        plan.orders[&CHILD_ID]
            .iter()
            .map(|component| component.id)
            .collect::<Vec<_>>(),
        ["child-shared", "child-consumer"]
    );
}

#[test]
fn reordered_transient_selection_plans_its_eager_dependencies() {
    static TRANSIENT_BEFORE_SCOPED: [ProviderOrder; 1] = [ProviderOrder {
        target: TypeDescriptor::of::<ScopedAlternativeProvider>("ScopedAlternativeProvider"),
        traits: &[],
        direction: ProviderOrderDirection::Before,
    }];
    let consumer = descriptor::<SelectedTransientConsumer>(
        "consumer",
        "SelectedTransientConsumer",
        &Singleton,
        reordered_provider_factory,
    );
    let dependency = descriptor::<SelectedTransientDependency>(
        "transient-dependency",
        "SelectedTransientDependency",
        &Singleton,
        empty_factory,
    );
    let scoped = descriptor::<ScopedAlternativeProvider>(
        "scoped-alternative",
        "ScopedAlternativeProvider",
        &Singleton,
        empty_factory,
    );
    let transient = descriptor::<SelectedTransientProvider>(
        "selected-transient",
        "SelectedTransientProvider",
        &upwell_core::Transient,
        transient_dependency_factory,
    );
    let descriptors = [consumer, scoped, dependency, transient];
    let selection = selection_with(
        &descriptors,
        vec![
            provider::<ScopedAlternativeProvider, dyn ReorderedProvider>("shared", 20, &[]),
            provider::<SelectedTransientProvider, dyn ReorderedProvider>(
                "shared",
                -20,
                &TRANSIENT_BEFORE_SCOPED,
            ),
        ],
    );
    let selected = selection.selected_dependencies_with_scope_reachability(
        &consumer,
        &reordered_provider_dependency(),
        |consumer, dependency| consumer == dependency,
    );
    let plan = ScopePlan::partition(&descriptors, &selection, &topology())
        .expect("selected transient dependency plans");
    let order = plan
        .singletons
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let planned = topological_sort(
        &plan.singletons,
        &HashSet::new(),
        &selection,
        |consumer, dependency| consumer == dependency,
    )
    .expect("root construction order plans")
    .into_iter()
    .map(|component| component.id)
    .collect::<Vec<_>>();

    assert!(matches!(
        selected.as_slice(),
        [upwell_di::SelectedDependency {
            target: DependencyTarget::Provider(provider),
            ..
        }] if provider.concrete_ty.type_id == TypeId::of::<SelectedTransientProvider>()
    ));
    assert_eq!(
        order,
        ["consumer", "scoped-alternative", "transient-dependency"]
    );
    assert!(
        planned.iter().position(|id| *id == "transient-dependency")
            < planned.iter().position(|id| *id == "consumer")
    );
}
