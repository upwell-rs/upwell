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
