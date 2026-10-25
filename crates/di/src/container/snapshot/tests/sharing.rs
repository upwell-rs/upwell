//! What a singleton snapshot shares with the active generation and what it copies.

use super::*;

#[tokio::test]
async fn arc_snapshot_shares_the_instance_but_not_the_live_cell() {
    let descriptor = ComponentDescriptor::of::<SharedService>();
    let active = Arc::new(SharedService { label: "active" });
    let root = root_with(&[descriptor], vec![seed(&descriptor, Arc::clone(&active))]).await;

    let snapshot = root
        .snapshot_singleton(descriptor)
        .expect("typed singleton snapshots");

    let active_cell = root
        .store
        .components
        .get(&descriptor.ty.type_id)
        .and_then(|boxed| boxed.downcast_ref::<Live<SharedService>>())
        .expect("active slot holds a Live cell");
    let snapshot_cell = snapshot
        .downcast_ref::<Live<SharedService>>()
        .expect("snapshot slot holds a Live cell");

    assert!(
        Arc::ptr_eq(&active_cell.snapshot(), &snapshot_cell.snapshot()),
        "the retained snapshot must preserve the active Arc instance"
    );

    active_cell.replace(Arc::new(SharedService { label: "replaced" }));

    assert!(
        !Arc::ptr_eq(&active_cell.snapshot(), &snapshot_cell.snapshot()),
        "generation-local Live cells must diverge after a replacement"
    );
    assert_eq!(snapshot_cell.snapshot().label, "active");
}

#[tokio::test]
async fn by_value_snapshot_reboxes_a_fresh_stored_value() {
    let descriptor = ComponentDescriptor::of::<ByValueService>();
    let root = root_with(
        &[descriptor],
        vec![seed(&descriptor, ByValueService { label: "seeded" })],
    )
    .await;

    let snapshot = root
        .snapshot_singleton(descriptor)
        .expect("by-value singleton snapshots");
    let snapshot_value = snapshot
        .downcast_ref::<ByValueService>()
        .expect("snapshot holds the by-value stored representation");

    assert_eq!(snapshot_value.label, "seeded");
    assert_eq!(snapshot.ty.type_id, descriptor.ty.type_id);
    assert_eq!(
        root.get::<ByValueService>()
            .expect("active slot resolves")
            .label,
        "seeded",
        "the active slot is untouched by the snapshot"
    );
}

#[tokio::test]
async fn a_custom_identity_descriptor_snapshots() {
    let descriptor =
        ComponentDescriptor::of::<SharedService>().with_identity("custom:shared", "CustomShared");
    let active = Arc::new(SharedService { label: "active" });
    let root = root_with(&[descriptor], vec![seed(&descriptor, Arc::clone(&active))]).await;

    let snapshot = root
        .snapshot_singleton(descriptor)
        .expect("custom-identity framework seeds snapshot");

    let snapshot_cell = snapshot
        .downcast_ref::<Live<SharedService>>()
        .expect("snapshot slot holds a Live cell");

    assert!(Arc::ptr_eq(&active, &snapshot_cell.snapshot()));
    assert_eq!(descriptor.id, "custom:shared");
}

#[tokio::test]
async fn plain_manual_descriptor_cannot_snapshot() {
    let typed = ComponentDescriptor::of::<SharedService>();
    let manual = ComponentDescriptor::manual(
        "snapshot-manual",
        "SharedService",
        TypeDescriptor::of::<SharedService>("SharedService"),
        Singleton::KEY,
    );
    let root = root_with(
        &[manual],
        vec![seed(&manual, Arc::new(SharedService { label: "active" }))],
    )
    .await;

    let error = root
        .snapshot_singleton(typed)
        .expect_err("candidate metadata cannot upgrade active manual provenance");

    assert!(matches!(error, Error::SnapshotUnavailable { .. }));
}

#[tokio::test]
async fn live_component_handle_cannot_share_its_slot_across_generations() {
    let descriptor = ComponentDescriptor::of::<LiveHandleComponent>();
    let root = root_with(
        &[descriptor],
        vec![seed(&descriptor, Live::new(Arc::new(LiveHandleComponent)))],
    )
    .await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("a Dep component handle cannot share its Live slot");

    assert!(matches!(error, Error::SnapshotHandleUnsupported { .. }));
}

#[tokio::test]
async fn snapshot_copies_no_ownership_or_provider_path() {
    let descriptor = ComponentDescriptor::of::<SharedService>();
    let root = root_with(
        &[descriptor],
        vec![seed(
            &descriptor,
            Arc::new(SharedService { label: "active" }),
        )],
    )
    .await;

    let components_before = root.store.components.len();
    let providers_before = root.store.providers.len();

    let snapshot = root
        .snapshot_singleton(descriptor)
        .expect("typed singleton snapshots");

    assert_eq!(
        root.store.components.len(),
        components_before,
        "the active root's concrete slots are untouched"
    );
    assert_eq!(
        root.store.providers.len(),
        providers_before,
        "the active root's provider aliases are untouched"
    );

    let mut fresh = ScopeStore::default();
    fresh.insert(snapshot);

    assert_eq!(
        fresh.components.len(),
        1,
        "the snapshot is exactly one concrete component"
    );
    assert!(
        fresh.providers.is_empty(),
        "the snapshot carries no provider aliases"
    );
}
