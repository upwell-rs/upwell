//! Tests for the hidden typed generation-snapshot metadata on
//! [`ComponentDescriptor`].

use upwell_core::{Singleton, TypeDescriptor};

use super::*;
use crate::test_support::test_component;

/// Minimal typed component used to probe descriptor metadata.
struct MetadataProbe;

test_component!(MetadataProbe, "metadata-probe");

#[test]
fn typed_descriptors_carry_the_generation_snapshot_adapter() {
    assert!(
        ComponentDescriptor::of::<MetadataProbe>()
            .generation_snapshot
            .is_some(),
        "of::<T>() descriptors must be snapshot-capable"
    );
    assert!(
        ComponentDescriptor::of::<MetadataProbe>()
            .with_identity("custom:probe", "CustomProbe")
            .generation_snapshot
            .is_some(),
        "custom-identity framework seeds must be snapshot-capable"
    );
}

#[test]
fn with_identity_renames_the_component_and_its_type() {
    let descriptor =
        ComponentDescriptor::of::<MetadataProbe>().with_identity("custom:probe", "CustomProbe");

    assert_eq!(descriptor.id, "custom:probe");
    assert_eq!(descriptor.name, "CustomProbe");
    assert_eq!(descriptor.ty.name, "CustomProbe");
    assert_eq!(
        descriptor.ty.type_id,
        std::any::TypeId::of::<MetadataProbe>()
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
