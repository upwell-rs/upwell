use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, ResolverSet, Scope, ScopeId, Singleton,
    Transient, TypeDescriptor,
};

use super::resolver_slot::ScopeResolverSlotState;
use super::*;
use crate::descriptors::component::{ComponentConstructionContext, from_boxed};
use crate::descriptors::{BoxedComponent, ComponentDescriptor, ProviderDescriptor};
use crate::error::Error;
use crate::registry::selection::rules::select_single_provider;
use crate::test_support::{no_dependencies, planned_provider, unerased};
use crate::{ComponentFactoryDescriptor, ComponentFactoryRole, Injectable};

/// A throwaway intermediate scope for exercising child-container construction
/// without depending on any protocol's concrete scopes.
struct TestScope;
struct SameNameScope;

impl Scope for TestScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/container");
    const RANK: u8 = 1;
    const NAME: &'static str = "Test";
}

impl Scope for SameNameScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/same-name");
    const RANK: u8 = 1;
    const NAME: &'static str = "Test";
}

const FRESH_TARGET_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-target");
const FRESH_OWNER_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-owner");

struct FreshTargetScope;

impl Scope for FreshTargetScope {
    const ID: ScopeId = FRESH_TARGET_SCOPE_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "FreshTarget";
}

struct FreshOwnerScope;

impl Scope for FreshOwnerScope {
    const ID: ScopeId = FRESH_OWNER_SCOPE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "FreshOwner";
}

trait ScopedProvider: Send + Sync {
    fn source(&self) -> &'static str;
}

struct TargetProvider;

impl ScopedProvider for TargetProvider {
    fn source(&self) -> &'static str {
        "target"
    }
}

struct OwnerProvider;

impl ScopedProvider for OwnerProvider {
    fn source(&self) -> &'static str {
        "owner"
    }
}

struct TargetSeed(&'static str);

struct FreshTarget {
    provider: Arc<dyn ScopedProvider>,
    seed: Arc<TargetSeed>,
}

fn fresh_target_dependencies() -> Vec<DependencyDescriptor> {
    vec![
        dependency::<dyn ScopedProvider>(false),
        dependency::<TargetSeed>(true),
    ]
}

fn dependency<T: ?Sized + 'static>(dynamic: bool) -> DependencyDescriptor {
    DependencyDescriptor {
        name: std::any::type_name::<T>(),
        ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        cardinality: Cardinality::One,
        optional: false,
        dynamic,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }
}

fn boxed_component<T: Send + Sync + 'static>(name: &'static str, value: Arc<T>) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<T>(name),
        value: Box::new(Injectable::into_stored(value)),
    }
}

fn target_provider_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Ok(boxed_component("TargetProvider", Arc::new(TargetProvider))) })
}

fn owner_provider_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Ok(boxed_component("OwnerProvider", Arc::new(OwnerProvider))) })
}

fn fresh_target_factory(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        let provider = cx
            .resolve::<Arc<dyn ScopedProvider>>()
            .await?
            .ok_or(Error::MissingComponent("ScopedProvider"))?;
        let seed = cx
            .resolve::<Arc<TargetSeed>>()
            .await?
            .ok_or(Error::MissingComponent("TargetSeed"))?;

        Ok(boxed_component(
            "FreshTarget",
            Arc::new(FreshTarget { provider, seed }),
        ))
    })
}

static TARGET_PROVIDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: target_provider_factory,
    construct_ready: None,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Explicit,
}];
static OWNER_PROVIDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: owner_provider_factory,
    construct_ready: None,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Explicit,
}];
static FRESH_TARGET_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fresh_target_factory,
    construct_ready: None,
    dependencies: fresh_target_dependencies,
    role: ComponentFactoryRole::Explicit,
}];

fn target_provider_factories() -> &'static [ComponentFactoryDescriptor] {
    &TARGET_PROVIDER_FACTORY
}

fn owner_provider_factories() -> &'static [ComponentFactoryDescriptor] {
    &OWNER_PROVIDER_FACTORY
}

fn fresh_target_factories() -> &'static [ComponentFactoryDescriptor] {
    &FRESH_TARGET_FACTORY
}

fn erase_provider<T: ScopedProvider + 'static>(component: &BoxedComponent) -> BoxedComponent {
    let concrete = from_boxed::<Arc<T>>(component).expect("provider stored");
    let erased: Arc<dyn ScopedProvider> = concrete;

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn ScopedProvider>("dyn ScopedProvider"),
        value: Box::new(Injectable::into_stored(erased)),
    }
}

fn erase_target_provider(component: &BoxedComponent) -> BoxedComponent {
    erase_provider::<TargetProvider>(component)
}

fn erase_owner_provider(component: &BoxedComponent) -> BoxedComponent {
    erase_provider::<OwnerProvider>(component)
}

fn provider_descriptor(
    concrete_ty: TypeDescriptor,
    erase: fn(&BoxedComponent) -> BoxedComponent,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn ScopedProvider>("dyn ScopedProvider"),
        concrete_ty,
        qualifier: "scoped",
        primary: false,
        priority: 0,
        ordering: &[],
        erase,
        live: |_| None,
    }
}

fn registry() -> Arc<ScopeRegistry> {
    Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    )
}

async fn root() -> Arc<ScopeContainer> {
    ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), registry())
        .await
        .expect("root builds")
}

mod children;
mod drop_guard;
mod fresh;
mod providers;
mod ready;
