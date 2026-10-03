//! Tests for the hidden typed generation-snapshot metadata on
//! [`ComponentDescriptor`].

use std::sync::Arc;

use upwell_core::{Singleton, TypeDescriptor};

use super::*;

/// Minimal typed component used to probe descriptor metadata.
struct MetadataProbe;

impl Component for MetadataProbe {
    type Handle = Arc<Self>;

    const ID: &'static str = "metadata-probe";
    const NAME: &'static str = "MetadataProbe";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[test]
fn typed_descriptors_carry_the_generation_snapshot_adapter() {
    assert!(
        ComponentDescriptor::of::<MetadataProbe>()
            .generation_snapshot
            .is_some(),
        "of::<T>() descriptors must be snapshot-capable"
    );
    assert!(
        ComponentDescriptor::manual_of::<MetadataProbe>("custom:probe", "CustomProbe", &Singleton,)
            .generation_snapshot
            .is_some(),
        "manual_of::<T>() framework seeds must be snapshot-capable"
    );
}

#[test]
fn raw_manual_descriptors_carry_no_snapshot_adapter() {
    let descriptor = ComponentDescriptor::manual(
        "raw:manual",
        "RawManual",
        TypeDescriptor::of::<MetadataProbe>("RawManual"),
        &Singleton,
    );

    assert!(
        descriptor.generation_snapshot.is_none(),
        "raw manual descriptors must never become retainable"
    );
}
