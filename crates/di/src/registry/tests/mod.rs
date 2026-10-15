use std::cell::Cell;

use super::*;
use crate::descriptors::{ComponentDescriptor, ComponentFactoryDescriptor};
use crate::test_support::{
    dependency, no_dependencies, planned, planned_provider, unbuilt, unbuilt_factories,
};
use upwell_core::{Cardinality, DependencyDescriptor, StaticScope, Transient, TypeDescriptor};

/// Local stand-in intermediate scopes (the captive rule only cares about rank
/// ordering): `Connection` outranks `Request`, both between singleton and transient.
/// They are defined here rather than imported so the DI engine stays unaware of any
/// protocol's concrete scopes.
struct Connection;
struct Request;

impl Scope for Connection {
    fn id(&self) -> ScopeId {
        ScopeId::new("test/connection").expect("valid test scope ID")
    }

    fn rank(&self) -> u8 {
        2
    }

    fn name(&self) -> &'static str {
        "Connection"
    }
}

impl Scope for Request {
    fn id(&self) -> ScopeId {
        ScopeId::new("test/request").expect("valid test scope ID")
    }

    fn rank(&self) -> u8 {
        1
    }

    fn name(&self) -> &'static str {
        "Request"
    }
}

/// Two distinct scopes that deliberately share a rank: `rank` is a lifetime
/// order, not an identity, so equal-rank siblings still resolve through
/// separate containers. Used to prove deferred selection groups by scope
/// identity rather than merging equal ranks.
struct SiblingA;
struct SiblingB;

impl StaticScope for SiblingA {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling-a");
    const RANK: u8 = 3;
    const NAME: &'static str = "SiblingA";
}

impl StaticScope for SiblingB {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling-b");
    const RANK: u8 = 3;
    const NAME: &'static str = "SiblingB";
}

/// A component named and identified `$name` of type `$ty` in `$scope`, whose one
/// never-driven recipe declares the `$dep` slice.
macro_rules! scoped {
    ($name:expr, $scope:expr, $dep:expr, $ty:expr $(,)?) => {
        planned($name, $ty, unbuilt_factories!("static", || $dep.to_vec())).with_scope($scope)
    };
}

fn descriptor_with_factories(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        name: "FactoryFixture",
        ..planned(
            "factory-fixture",
            TypeDescriptor::of::<u128>("FactoryFixture"),
            factories,
        )
    }
}

/// The non-primary mapping of `concrete` to the shared `dyn Send` stand-in trait.
fn trait_provider(
    concrete: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        primary,
        ..planned_provider(
            TypeDescriptor::of::<dyn Send>("SharedTrait"),
            concrete,
            qualifier,
        )
    }
}

mod captive;
mod deferred;
mod factories;
mod fresh;
mod reachability;
mod validation;
