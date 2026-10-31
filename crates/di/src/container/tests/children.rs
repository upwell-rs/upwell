//! Child scope construction and component-source lifetime.

use super::*;
use crate::test_support::test_component;

#[tokio::test]
async fn empty_child_scope_retains_its_identity() {
    let root = root().await;
    let registry = registry();

    let child = ScopeContainer::open_child(
        TestScope::KEY,
        Arc::clone(&root),
        Arc::clone(&registry),
        &[],
        Vec::new(),
    )
    .await
    .expect("open child");

    assert!(!Arc::ptr_eq(&root, &child));
    assert_eq!(child.scope().id(), TestScope::ID);
    assert!(child.belongs_to_registry(&registry));
    assert!(!child.belongs_to_registry(&self::registry()));
    assert!(!child.can_access(SameNameScope::KEY));
    assert!(matches!(
        child.slot.state,
        ScopeResolverSlotState::Attached(_)
    ));
    assert!(Arc::ptr_eq(
        &child,
        &child.slot.resolve().expect("attached scope resolves")
    ));

    let source = child
        .resolvers()
        .get_arc::<ComponentSource>()
        .expect("empty child exposes its component source");

    assert!(Arc::ptr_eq(
        &child,
        &source.container.upgrade().expect("child remains alive")
    ));
}

#[tokio::test]
async fn child_scope_with_a_seed_is_built() {
    let root = root().await;

    let seed = BoxedComponent {
        ty: TypeDescriptor::of::<u8>("u8"),
        value: Box::new(7u8),
    };

    let child = ScopeContainer::open_child(
        TestScope::KEY,
        Arc::clone(&root),
        registry(),
        &[],
        vec![seed],
    )
    .await
    .expect("open child");

    assert!(
        !Arc::ptr_eq(&root, &child),
        "a seeded scope should allocate its own container"
    );
    assert_eq!(child.scope().name(), "Test");
}

#[tokio::test]
async fn component_source_does_not_keep_a_scope_alive() {
    let root = root().await;
    let source = root
        .resolvers()
        .get_arc::<ComponentSource>()
        .expect("component source installed");

    drop(root);

    assert!(
        source.component::<NeverRegistered>().is_none(),
        "a weak component source must not retain its container"
    );
}

/// A component no registry knows about.
struct NeverRegistered;

test_component!(NeverRegistered, "never-registered");
