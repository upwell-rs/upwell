//! Slot layout planning over marker descriptors.
//!
//! The shared fixture lives here; `planning` covers the source each edge plans to and
//! `positions` covers reading planned edges by declared position.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton,
    Transient, TypeDescriptor,
};

use super::*;
use crate::descriptors::{ComponentDescriptor, ProviderDescriptor};
use crate::error::Error;
use crate::registry::ProviderSelectionModel;
use crate::test_support::{component, dependency, provider, unbuilt_factories};

struct Outer;
struct Inner;
struct Sibling;

impl Scope for Outer {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-outer");
    const RANK: u8 = 2;
    const NAME: &'static str = "Outer";
}

impl Scope for Inner {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-inner");
    const RANK: u8 = 1;
    const NAME: &'static str = "Inner";
}

impl Scope for Sibling {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/layout-sibling");
    const RANK: u8 = 1;
    const NAME: &'static str = "Sibling";
}

trait Marker {}

struct Shared;
struct Connection;
struct Seed;
struct Handler;
struct Fresh;
struct Consumer;
struct Elsewhere;

fn marker() -> TypeDescriptor {
    TypeDescriptor::of::<dyn Marker>("Marker")
}

fn eager<T: 'static>(name: &'static str, cardinality: Cardinality) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(name),
        cardinality,
        ResolutionMode::Eager,
    )
}

fn lazy<T: 'static>(name: &'static str) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(name),
        Cardinality::One,
        ResolutionMode::Lazy,
    )
}

fn on_marker(cardinality: Cardinality) -> DependencyDescriptor {
    dependency("Marker", marker(), cardinality, ResolutionMode::Eager)
}

fn scoped(mut descriptor: ComponentDescriptor, scope: ScopeKey) -> ComponentDescriptor {
    descriptor.scope = scope;

    descriptor
}

/// A component of `$ty` with id `$id` in `$scope`, whose one unbuilt factory declares the
/// edges `$dependencies` returns.
macro_rules! scoped_component {
    ($ty:ty, $id:literal, $scope:expr, $dependencies:expr $(,)?) => {
        scoped(
            component::<$ty>($id, unbuilt_factories!($id, $dependencies)),
            $scope,
        )
    };
}

/// The fixture graph: a root `Shared`, an outer `Connection`, an inner seed and handler,
/// a transient `Fresh`, and an inner `Consumer` of every edge shape.
struct Fixture {
    shared: ComponentDescriptor,
    connection: ComponentDescriptor,
    seed: ComponentDescriptor,
    handler: ComponentDescriptor,
    fresh: ComponentDescriptor,
    consumer: ComponentDescriptor,
    providers: Vec<ProviderDescriptor>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            shared: component::<Shared>("shared", unbuilt_factories!("shared", Vec::new)),
            connection: scoped_component!(Connection, "connection", Outer::KEY, || vec![eager::<
                Shared,
            >(
                "Shared",
                Cardinality::One
            )]),
            seed: scoped(component::<Seed>("seed", || &[]), Inner::KEY),
            handler: scoped_component!(Handler, "handler", Inner::KEY, || vec![
                eager::<Connection>("Connection", Cardinality::One),
                eager::<Seed>("Seed", Cardinality::One),
            ]),
            fresh: scoped_component!(Fresh, "fresh", Transient::KEY, || vec![eager::<Shared>(
                "Shared",
                Cardinality::One
            )]),
            consumer: scoped_component!(Consumer, "consumer", Inner::KEY, || vec![
                eager::<Handler>("Handler", Cardinality::One),
                eager::<Fresh>("Fresh", Cardinality::One),
                on_marker(Cardinality::One),
                on_marker(Cardinality::Collection),
            ]),
            providers: vec![
                provider::<Handler>(marker(), "handler", true),
                provider::<Shared>(marker(), "shared", false),
            ],
        }
    }

    fn selection(&self, extra: &[ComponentDescriptor]) -> ProviderSelectionModel {
        let mut components = vec![
            self.shared,
            self.connection,
            self.seed,
            self.handler,
            self.fresh,
            self.consumer,
        ];

        components.extend_from_slice(extra);

        ProviderSelectionModel::new(&components, self.providers.clone(), HashMap::new())
            .expect("the fixture providers name fixture components")
    }

    fn outer(&self, selection: &ProviderSelectionModel) -> Arc<SlotLayout> {
        let layout = SlotLayout::plan(
            Outer::KEY,
            &[],
            &[self.connection],
            &HashMap::new(),
            selection,
            reach,
        )
        .expect("the outer scope plans");

        Arc::new(layout)
    }

    fn inner(
        &self,
        selection: &ProviderSelectionModel,
        order: &[ComponentDescriptor],
    ) -> crate::Result<SlotLayout> {
        let ancestors = HashMap::from([(Outer::ID, self.outer(selection))]);

        SlotLayout::plan(
            Inner::KEY,
            &[self.seed],
            order,
            &ancestors,
            selection,
            reach,
        )
    }
}

/// `Inner` opens only under `Outer`, and every scope reaches the root.
fn reach(consumer: ScopeId, dependency: ScopeId) -> bool {
    dependency == Singleton::ID
        || consumer == dependency
        || (consumer == Inner::ID && dependency == Outer::ID)
}

fn sources(layout: &SlotLayout, slot: usize) -> Vec<&DepRef> {
    layout.slots()[slot]
        .dependencies
        .iter()
        .map(|edge| &edge.source)
        .collect()
}

mod planning;
mod positions;
