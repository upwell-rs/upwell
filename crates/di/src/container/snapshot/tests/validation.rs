//! Snapshot rejections, and redaction of storage and payload failures.

use super::*;

#[tokio::test]
async fn snapshot_rejects_a_non_singleton_descriptor() {
    let descriptor = ComponentDescriptor::manual_of::<SharedService>(
        "snapshot-shared",
        "SharedService",
        &SnapshotRequestScope,
    );
    let typed = ComponentDescriptor::of::<SharedService>();
    let root = root_with(
        &[typed],
        vec![seed(&typed, Arc::new(SharedService { label: "active" }))],
    )
    .await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("only singleton descriptors snapshot");

    assert!(matches!(error, Error::SnapshotUnavailable { .. }));
}

#[tokio::test]
async fn snapshot_requires_the_component_in_the_local_store() {
    let descriptor = ComponentDescriptor::of::<SharedService>();
    let root = root_with(&[], Vec::new()).await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("an absent component cannot snapshot");

    assert!(matches!(error, Error::MissingComponent("SharedService")));
}

#[tokio::test]
async fn snapshot_storage_mismatch_is_redacted() {
    let descriptor = ComponentDescriptor::of::<SharedService>();
    let mismatched = BoxedComponent {
        ty: descriptor.ty,
        value: Box::new(ByValueService { label: "wrong" }),
    };
    let root = root_with(&[descriptor], vec![mismatched]).await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("incompatible active storage is rejected");

    assert!(matches!(error, Error::SnapshotStorageMismatch { .. }));
}

#[tokio::test]
async fn snapshot_panics_are_redacted() {
    let descriptor = ComponentDescriptor::of::<PanickingComponent>();
    let root = root_with(&[descriptor], vec![seed(&descriptor, PanickingHandle)]).await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("panicking adapters are contained");

    assert!(matches!(error, Error::SnapshotPanicked { .. }));
    assert!(
        !error.to_string().contains("canary"),
        "panic payloads must be redacted"
    );
}

#[tokio::test]
async fn snapshot_output_type_is_validated() {
    let mut descriptor = ComponentDescriptor::of::<SharedService>();
    descriptor.generation_snapshot =
        Some(crate::descriptors::component::GenerationSnapshot::for_test(
            |_| {
                Ok(BoxedComponent {
                    ty: TypeDescriptor::of::<ByValueService>("ByValueService"),
                    value: Box::new(Injectable::into_stored(ByValueService { label: "alien" })),
                })
            },
            |_| true,
        ));

    let root = root_with(
        &[descriptor],
        vec![seed(
            &descriptor,
            Arc::new(SharedService { label: "active" }),
        )],
    )
    .await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("mismatched snapshot output is rejected");

    assert!(matches!(error, Error::SnapshotOutputMismatch { .. }));
}

#[tokio::test]
async fn snapshot_output_payload_is_validated() {
    let mut descriptor = ComponentDescriptor::of::<SharedService>();
    descriptor.generation_snapshot =
        Some(crate::descriptors::component::GenerationSnapshot::for_test(
            |active| {
                Ok(BoxedComponent {
                    ty: active.ty,
                    value: Box::new(ByValueService { label: "alien" }),
                })
            },
            |candidate| candidate.downcast_ref::<Live<SharedService>>().is_some(),
        ));
    let root = root_with(
        &[descriptor],
        vec![seed(
            &descriptor,
            Arc::new(SharedService { label: "active" }),
        )],
    )
    .await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("mismatched snapshot payload is rejected");

    assert!(matches!(error, Error::SnapshotOutputMismatch { .. }));
}
