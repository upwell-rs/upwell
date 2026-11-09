//! One component type placed in two disjoint scopes: each scope builds and serves its own.

use super::*;
use crate::test_support::{component, test_component};
use crate::{dispatch_factory, factory_dependencies};

/// A component with one factory per scope.
struct Twin {
    source: &'static str,
}

test_component!(Twin, "twin");

async fn in_test_scope() -> Twin {
    Twin { source: "test" }
}

async fn in_same_name_scope() -> Twin {
    Twin {
        source: "same-name",
    }
}

/// Declares the placement function `$name` of [`Twin`] in `$scope`, built by `$factory`.
macro_rules! placement {
    ($name:ident, $scope:ty, $id:literal, $factory:ident) => {
        fn $name() -> ComponentDescriptor {
            fn factories() -> &'static [ComponentFactoryDescriptor] {
                const {
                    &[ComponentFactoryDescriptor {
                        id: $id,
                        construct: |cx| dispatch_factory($factory, cx),
                        construct_ready: None,
                        dependencies: || factory_dependencies($factory),
                        role: ComponentFactoryRole::Explicit,
                        scopes: &[<$scope>::KEY],
                    }]
                }
            }

            let mut descriptor = component::<Twin>($id, factories);

            descriptor.scope = <$scope>::KEY;
            descriptor.declared_scope = Some(<$scope>::KEY);

            descriptor
        }
    };
}

placement!(twin_in_test_scope, TestScope, "twin@test", in_test_scope);
placement!(
    twin_in_same_name_scope,
    SameNameScope,
    "twin@same-name",
    in_same_name_scope
);

fn layout_of(
    scope: upwell_core::ScopeKey,
    placement: ComponentDescriptor,
) -> Arc<crate::SlotLayout> {
    let order = [placement];
    let selection =
        crate::registry::ProviderSelectionModel::new(&order, Vec::new(), HashMap::new())
            .expect("the placement selects no providers");

    Arc::new(
        crate::SlotLayout::plan(scope, &[], &order, &HashMap::new(), &selection, |_, _| true)
            .expect("the scope plans"),
    )
}

async fn open_twin(layout: &Arc<crate::SlotLayout>) -> &'static str {
    let scope = ScopeContainer::open_planned(
        root().await,
        registry(),
        layout,
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens");

    scope
        .resolve::<Arc<Twin>>()
        .await
        .expect("the twin resolves")
        .expect("the scope holds a twin")
        .source
}

#[tokio::test]
async fn each_scope_builds_the_factory_declared_for_it() {
    let test = layout_of(TestScope::KEY, twin_in_test_scope());
    let same_name = layout_of(SameNameScope::KEY, twin_in_same_name_scope());

    assert_eq!(open_twin(&test).await, "test");
    assert_eq!(open_twin(&same_name).await, "same-name");
}

#[test]
fn selection_resolves_each_consumer_to_the_placement_it_reaches() {
    let placements = [twin_in_test_scope(), twin_in_same_name_scope()];
    let selection =
        crate::registry::ProviderSelectionModel::new(&placements, Vec::new(), HashMap::new())
            .expect("the placements select no providers");
    let same_scope =
        |consumer: upwell_core::ScopeKey, dependency: upwell_core::ScopeKey| consumer == dependency;
    let type_id = TypeId::of::<Twin>();

    let from_test = selection.component_from(type_id, TestScope::KEY, &same_scope);
    let from_same_name = selection.component_from(type_id, SameNameScope::KEY, &same_scope);

    assert_eq!(from_test.map(|c| c.scope.id()), Some(TestScope::ID));
    assert_eq!(
        from_same_name.map(|c| c.scope.id()),
        Some(SameNameScope::ID)
    );
    assert_eq!(selection.placements(type_id).len(), 2);
}
