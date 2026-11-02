//! A scope opened from a layout allocates a resolver slot only when a factory captures it.

use super::*;
use crate::test_support::{component, test_component};
use crate::{Lazy, dispatch_factory, factory_dependencies};

/// A transient built for every resolution.
struct Made;

/// A scoped component reading [`Made`] through a planned transient edge.
struct UsesMade {
    _made: Arc<Made>,
}

/// A scoped component capturing its scope through a lazy handle.
struct LazyUser {
    uses: Lazy<Arc<UsesMade>>,
}

/// A transient capturing the requesting scope through a lazy handle.
struct LazyMade {
    made: Lazy<Arc<Made>>,
}

/// A scoped component reading [`LazyMade`] through a planned transient edge.
struct HoldsLazyMade {
    made: Arc<LazyMade>,
}

/// A transient capturing the requesting scope through a lazy handle its factory does not
/// declare.
struct HiddenLazyMade {
    made: Lazy<Arc<Made>>,
}

/// A scoped component reading [`HiddenLazyMade`] through a planned transient edge.
struct HoldsHidden {
    made: Arc<HiddenLazyMade>,
}

test_component!(Made, "made");
test_component!(LazyMade, "lazy-made");
test_component!(HoldsLazyMade, "holds-lazy-made");
test_component!(HiddenLazyMade, "hidden-lazy-made");
test_component!(HoldsHidden, "holds-hidden");
test_component!(UsesMade, "uses-made");
test_component!(LazyUser, "lazy-user");

async fn made() -> Made {
    Made
}

async fn uses_made(made: Arc<Made>) -> UsesMade {
    UsesMade { _made: made }
}

async fn lazy_made(made: Lazy<Arc<Made>>) -> LazyMade {
    LazyMade { made }
}

async fn holds_lazy_made(made: Arc<LazyMade>) -> HoldsLazyMade {
    HoldsLazyMade { made }
}

async fn lazy_user(uses: Lazy<Arc<UsesMade>>) -> LazyUser {
    LazyUser { uses }
}

async fn holds_hidden(made: Arc<HiddenLazyMade>) -> HoldsHidden {
    HoldsHidden { made }
}

fn construct_hidden(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async move {
        let made = <Lazy<Arc<Made>> as crate::FromContainer>::from_container(cx).await?;

        Ok(boxed_component(
            "HiddenLazyMade",
            Arc::new(HiddenLazyMade { made }),
        ))
    })
}

fn hidden_descriptor() -> ComponentDescriptor {
    fn factories() -> &'static [ComponentFactoryDescriptor] {
        const {
            &[ComponentFactoryDescriptor {
                id: "hidden-lazy-made",
                construct: construct_hidden,
                construct_ready: None,
                dependencies: crate::test_support::no_dependencies,
                role: ComponentFactoryRole::Explicit,
            }]
        }
    }

    let mut descriptor = component::<HiddenLazyMade>("hidden-lazy-made", factories);

    descriptor.scope = Transient::KEY;

    descriptor
}

/// Declares descriptor function `$name` of component `$ty` in `$scope`, built by `$factory`.
macro_rules! described {
    ($name:ident, $ty:ty, $id:literal, $scope:expr, $factory:ident) => {
        fn $name() -> ComponentDescriptor {
            fn factories() -> &'static [ComponentFactoryDescriptor] {
                const {
                    &[ComponentFactoryDescriptor {
                        id: $id,
                        construct: |cx| dispatch_factory($factory, cx),
                        construct_ready: None,
                        dependencies: || factory_dependencies($factory),
                        role: ComponentFactoryRole::Explicit,
                    }]
                }
            }

            let mut descriptor = component::<$ty>($id, factories);

            descriptor.scope = $scope;

            descriptor
        }
    };
}

described!(made_descriptor, Made, "made", Transient::KEY, made);
described!(
    uses_descriptor,
    UsesMade,
    "uses-made",
    TestScope::KEY,
    uses_made
);
described!(
    lazy_descriptor,
    LazyUser,
    "lazy-user",
    TestScope::KEY,
    lazy_user
);
described!(
    lazy_made_descriptor,
    LazyMade,
    "lazy-made",
    Transient::KEY,
    lazy_made
);
described!(
    holds_descriptor,
    HoldsLazyMade,
    "holds-lazy-made",
    TestScope::KEY,
    holds_lazy_made
);
described!(
    holds_hidden_descriptor,
    HoldsHidden,
    "holds-hidden",
    TestScope::KEY,
    holds_hidden
);

/// Opens a `TestScope` building `order`, with the transients above registered.
async fn open(order: &[ComponentDescriptor]) -> (Arc<ScopeContainer>, bool) {
    let transients = [
        made_descriptor(),
        lazy_made_descriptor(),
        hidden_descriptor(),
    ];
    let known = [transients.as_slice(), order].concat();

    let selection = Arc::new(
        crate::registry::ProviderSelectionModel::new(&known, Vec::new(), HashMap::new())
            .expect("the components select no providers"),
    );
    let layout = crate::SlotLayout::plan(
        TestScope::KEY,
        &[],
        order,
        &HashMap::new(),
        &selection,
        |_, _| true,
    )
    .expect("the scope plans");
    let shares_store = layout.shares_store();
    let components = order
        .iter()
        .map(|component| (component.ty.type_id, *component))
        .collect();
    let registry = ScopeRegistry::from_selection_model(
        transients
            .iter()
            .map(|transient| (transient.ty.type_id, *transient))
            .collect(),
        components,
        selection,
    )
    .expect("the registry validates");
    let scope = ScopeContainer::open_planned(
        root().await,
        Arc::new(registry),
        &Arc::new(layout),
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens");

    (scope, shares_store)
}

#[tokio::test]
async fn a_scope_reading_a_planned_transient_needs_no_resolver_slot_or_shared_store() {
    let (scope, shares_store) = open(&[uses_descriptor()]).await;

    assert!(
        scope.get::<UsesMade>().is_some(),
        "the transient edge resolved"
    );
    assert!(
        !shares_store,
        "the scope is built into an owned store, read without a lock"
    );
    assert!(
        matches!(scope.slot.state, ScopeResolverSlotState::Attached(_)),
        "no factory captured the scope, so no pending resolver slot was allocated"
    );
}

#[tokio::test]
async fn a_scope_captured_by_a_lazy_handle_gets_a_resolver_slot() {
    let (scope, _) = open(&[uses_descriptor(), lazy_descriptor()]).await;
    let user = scope
        .get::<LazyUser>()
        .expect("the scope built the lazy user");

    assert!(
        matches!(scope.slot.state, ScopeResolverSlotState::Pending(_)),
        "the lazy handle captured a pending resolver slot"
    );

    user.uses
        .get_or_create()
        .await
        .expect("the captured scope resolves its sibling");
}

#[tokio::test]
async fn a_planned_transient_capturing_its_requesting_scope_gets_its_resolver_slot() {
    let (scope, _) = open(&[uses_descriptor(), holds_descriptor()]).await;
    let holder = scope
        .get::<HoldsLazyMade>()
        .expect("the scope built the holder");

    assert!(
        matches!(scope.slot.state, ScopeResolverSlotState::Pending(_)),
        "the transient's lazy handle captured a pending resolver slot"
    );

    holder
        .made
        .made
        .get_or_create()
        .await
        .expect("the transient's lazy handle resolves from the requesting scope");
}

#[tokio::test]
async fn a_planned_transient_capturing_its_scope_without_declaring_it_still_resolves() {
    let (scope, _) = open(&[uses_descriptor(), holds_hidden_descriptor()]).await;
    let holder = scope
        .get::<HoldsHidden>()
        .expect("the scope built the holder");

    assert!(
        matches!(scope.slot.state, ScopeResolverSlotState::Pending(_)),
        "the scope adopted the slot the transient captured"
    );

    holder
        .made
        .made
        .get_or_create()
        .await
        .expect("the undeclared capture resolves from the requesting scope");
}
