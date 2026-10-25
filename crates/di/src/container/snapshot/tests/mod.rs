//! Tests for [`ScopeContainer::snapshot_singleton`].

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, Scope, ScopeId, Singleton, TypeDescriptor};

use crate::container::ScopeContainer;
use crate::test_support::test_component;
use crate::{
    BoxedComponent, Component, ComponentDescriptor, Error, Injectable, Live, ScopeRegistry,
    descriptors::component::ScopeStore,
};

struct SharedService {
    label: &'static str,
}

test_component!(SharedService, "snapshot-shared");

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

    fn snapshot_stored(stored: &Self) -> Option<Self> {
        Some(stored.clone())
    }
}

struct PanickingComponent;

struct LiveHandleComponent;

impl Component for LiveHandleComponent {
    type Handle = crate::Dep<Self>;

    const ID: &'static str = "snapshot-live-handle";
    const NAME: &'static str = "LiveHandleComponent";

    fn into_handle(self) -> Self::Handle {
        Live::new(Arc::new(self))
    }
}

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

    fn snapshot_stored(_: &Arc<PanickingComponent>) -> Option<Arc<PanickingComponent>> {
        panic!("snapshot canary")
    }
}

struct SnapshotRequestScope;

impl Scope for SnapshotRequestScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/snapshot-request");
    const RANK: u8 = 1;
    const NAME: &'static str = "SnapshotRequest";
}

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

mod sharing;
mod validation;
