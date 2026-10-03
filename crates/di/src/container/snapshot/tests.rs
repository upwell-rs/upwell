//! Tests for [`ScopeContainer::snapshot_singleton`].

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, ScopeId, Singleton, StaticScope, TypeDescriptor};

use crate::container::ScopeContainer;
use crate::{
    BoxedComponent, Component, ComponentDescriptor, Error, Injectable, Live, ScopeRegistry,
    descriptors::component::ScopeStore,
};

// ---------------------------------------------------------------------------
// Test components.
// ---------------------------------------------------------------------------

struct SharedService {
    label: &'static str,
}

impl Component for SharedService {
    type Handle = Arc<Self>;

    const ID: &'static str = "snapshot-shared";
    const NAME: &'static str = "SharedService";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

struct ByValueService {
    label: &'static str,
}

impl Clone for ByValueService {
    fn clone(&self) -> Self {
        Self { label: self.label }
    }
}

impl Component for ByValueService {
    type Handle = Self;

    const ID: &'static str = "snapshot-by-value";
    const NAME: &'static str = "ByValueService";

    fn into_handle(self) -> Self {
        self
    }
}

impl Injectable for ByValueService {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

struct PanickingComponent;

#[derive(Clone)]
struct PanickingHandle;

impl Component for PanickingComponent {
    type Handle = PanickingHandle;

    const ID: &'static str = "snapshot-panicking";
    const NAME: &'static str = "PanickingComponent";

    fn into_handle(self) -> PanickingHandle {
        PanickingHandle
    }
}

impl Injectable for PanickingHandle {
    type Target = PanickingComponent;
    type Stored = Arc<PanickingComponent>;

    fn into_stored(self) -> Arc<PanickingComponent> {
        Arc::new(PanickingComponent)
    }

    fn from_stored(_: &Arc<PanickingComponent>) -> Self {
        panic!("snapshot canary")
    }
}

struct SnapshotRequestScope;

impl StaticScope for SnapshotRequestScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/snapshot-request");
    const RANK: u8 = 1;
    const NAME: &'static str = "SnapshotRequest";
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn registry(descriptors: &[ComponentDescriptor]) -> Arc<ScopeRegistry> {
    let components = descriptors
        .iter()
        .map(|descriptor| (descriptor.ty.type_id, *descriptor))
        .collect();

    Arc::new(
        ScopeRegistry::new(HashMap::new(), components, Vec::new(), HashMap::new())
            .expect("snapshot registry validates"),
    )
}

fn seed<H: Injectable>(descriptor: &ComponentDescriptor, handle: H) -> BoxedComponent {
    BoxedComponent {
        ty: descriptor.ty,
        value: Box::new(Injectable::into_stored(handle)),
    }
}

async fn root_with(
    descriptors: &[ComponentDescriptor],
    seeds: Vec<BoxedComponent>,
) -> Arc<ScopeContainer> {
    ScopeContainer::build_root(
        descriptors,
        seeds,
        ResolverSet::new(),
        registry(descriptors),
    )
    .await
    .expect("root builds")
}

// ---------------------------------------------------------------------------
// Snapshot behavior.
// ---------------------------------------------------------------------------

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
async fn manual_of_descriptor_snapshots_with_a_custom_identity() {
    let descriptor = ComponentDescriptor::manual_of::<SharedService>(
        "custom:shared",
        "CustomShared",
        &Singleton,
    );
    let active = Arc::new(SharedService { label: "active" });
    let root = root_with(&[descriptor], vec![seed(&descriptor, Arc::clone(&active))]).await;

    let snapshot = root
        .snapshot_singleton(descriptor)
        .expect("manual_of framework seeds snapshot");

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
        &Singleton,
    );
    let root = root_with(
        &[typed],
        vec![seed(&typed, Arc::new(SharedService { label: "active" }))],
    )
    .await;

    let error = root
        .snapshot_singleton(manual)
        .expect_err("raw manual descriptors are not retainable");

    assert!(matches!(error, Error::SnapshotUnavailable { .. }));
}

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
    descriptor.generation_snapshot = Some(|_: &BoxedComponent| {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ByValueService>("ByValueService"),
            value: Box::new(Injectable::into_stored(ByValueService { label: "alien" })),
        })
    });

    let typed = ComponentDescriptor::of::<SharedService>();
    let root = root_with(
        &[typed],
        vec![seed(&typed, Arc::new(SharedService { label: "active" }))],
    )
    .await;

    let error = root
        .snapshot_singleton(descriptor)
        .expect_err("mismatched snapshot output is rejected");

    assert!(matches!(error, Error::SnapshotOutputMismatch { .. }));
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
