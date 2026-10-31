//! How a scope stores the handles its factories build.

use super::*;
use crate::test_support::{component, test_component};
use crate::{Dep, Live, dispatch_factory, factory_dependencies};

/// A component built through the `Factory` machinery.
struct Held;

test_component!(Held, "held");

async fn held() -> Held {
    Held
}

fn factories() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "held",
            construct: |cx| dispatch_factory(held, cx),
            construct_ready: None,
            dependencies: || factory_dependencies(held),
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn stored(container: &ScopeContainer) -> &Live<Held> {
    container
        .store
        .component(TypeId::of::<Held>())
        .and_then(|boxed| boxed.value.downcast_ref::<Live<Held>>())
        .expect("the scope stores the built component")
}

#[tokio::test]
async fn the_root_stores_singletons_in_swappable_cells() {
    let descriptor = component::<Held>("held", factories);

    let root =
        ScopeContainer::build_root(&[descriptor], Vec::new(), ResolverSet::new(), registry())
            .await
            .expect("root builds");

    assert!(
        <Arc<Held> as Injectable>::live_cell(stored(&root)).is_some(),
        "a transition can republish a singleton"
    );
}

#[tokio::test]
async fn a_child_scope_stores_fixed_handles() {
    let mut descriptor = component::<Held>("held", factories);

    descriptor.scope = TestScope::KEY;

    let child = ScopeContainer::open_child(
        TestScope::KEY,
        root().await,
        registry(),
        &[descriptor],
        Vec::new(),
    )
    .await
    .expect("child opens");
    let built = child
        .resolve::<Arc<Held>>()
        .await
        .expect("resolution succeeds")
        .expect("the child holds the component");
    let dep = child
        .resolve::<Dep<Held>>()
        .await
        .expect("resolution succeeds")
        .expect("the child holds the component");

    assert!(
        <Arc<Held> as Injectable>::live_cell(stored(&child)).is_none(),
        "no transition rebinds an instance of a scope below the root"
    );
    assert!(
        Arc::ptr_eq(&built, &dep.snapshot()),
        "a live handle onto a fixed slot reads the same instance"
    );
}

/// A hand-written component that declares no dependencies but captures its scope.
struct Capturing {
    held: crate::Lazy<Arc<Held>>,
}

test_component!(Capturing, "capturing");

fn construct_capturing(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async move {
        let held = <crate::Lazy<Arc<Held>> as crate::FromContainer>::from_container(cx).await?;

        Ok(boxed_component("Capturing", Arc::new(Capturing { held })))
    })
}

fn capturing_factories() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "capturing",
            construct: construct_capturing,
            construct_ready: None,
            dependencies: no_dependencies,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

#[tokio::test]
async fn a_factory_capturing_a_scope_its_layout_did_not_plan_for_still_resolves() {
    let mut held = component::<Held>("held", factories);
    let mut capturing = component::<Capturing>("capturing", capturing_factories);

    held.scope = TestScope::KEY;
    capturing.scope = TestScope::KEY;

    let layout = plan(&[held, capturing]);

    assert!(!layout.captures(), "no declared edge captures the scope");

    let scope = ScopeContainer::open_planned(
        root().await,
        registry(),
        &Arc::new(layout),
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens");
    let capturing = scope
        .resolve::<Arc<Capturing>>()
        .await
        .expect("resolution succeeds")
        .expect("the scope holds the capturing component");

    capturing
        .held
        .get_or_create()
        .await
        .expect("the captured scope resolves its sibling");
}

#[tokio::test]
async fn a_seed_for_a_component_a_factory_builds_is_rejected() {
    let mut held = component::<Held>("held", factories);

    held.scope = TestScope::KEY;

    let layout = plan(&[held]);

    let error = match ScopeContainer::open_planned(
        root().await,
        registry(),
        &Arc::new(layout),
        None,
        vec![boxed_component("Held", Arc::new(Held))],
        Arc::new(()),
    )
    .await
    {
        Ok(_) => panic!("a seed for a factory-built component was accepted"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::SeedForBuiltComponent { .. }));
}
