use upwell_core::{
    Cardinality, DependencyDescriptor, Singleton, StaticScope, Transient, TypeDescriptor,
};

use super::*;
use crate::descriptors::ComponentFactoryDescriptor;
use crate::error::Error;
use crate::registry::ComponentRegistry;
use crate::test_support::{component, dependency, no_dependencies, provider, unbuilt_factories};

const NEAR_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/selection-near");
const FAR_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/selection-far");

struct NearScope;

impl StaticScope for NearScope {
    const ID: ScopeId = NEAR_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Near";
}

struct FarScope;

impl StaticScope for FarScope {
    const ID: ScopeId = FAR_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "Far";
}

/// The `dyn Send` stand-in trait the selection tests map providers to.
const SELECTED_TRAIT: TypeDescriptor = TypeDescriptor::of::<dyn Send>("SelectedTrait");

struct Consumer;
struct Near;
struct Far;
struct TransientProvider;
struct Manual;

fn factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("static", no_dependencies)()
}

fn manual() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

fn selected_trait(
    cardinality: Cardinality,
    resolution: ResolutionMode,
    qualifier: Option<&'static str>,
) -> DependencyDescriptor {
    DependencyDescriptor {
        qualifier,
        ..dependency("SelectedTrait", SELECTED_TRAIT, cardinality, resolution)
    }
}

fn reaches(consumer: ScopeId, dependency: ScopeId) -> bool {
    consumer == dependency || dependency == FAR_ID || dependency == Singleton.id()
}

fn selected_types(selected: &[SelectedDependency]) -> Vec<TypeId> {
    selected
        .iter()
        .map(|selected| match selected.target {
            DependencyTarget::Component(component) => component.ty.type_id,
            DependencyTarget::Provider(provider) => provider.concrete_ty.type_id,
        })
        .collect()
}

mod eager;
mod fresh;
