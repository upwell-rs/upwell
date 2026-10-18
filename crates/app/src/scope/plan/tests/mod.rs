use std::any::TypeId;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, StaticScope, TypeDescriptor,
};
use upwell_di::{
    ComponentFactoryDescriptor, ComponentRegistry, DependencyTarget, ProviderDescriptor,
    ProviderOrder, ProviderOrderDirection, ProviderSelectionModel, Singleton,
};

use super::*;
use crate::test_support::{dependency, planned_provider, unbuilt_factories};
use crate::{Error, ScopeBoundary, ScopeParent, ScopeTopology};

const PARENT_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/parent");
const CHILD_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/child");
const SIBLING_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling");
const MISSING_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/missing");

struct Parent;
struct Child;
struct Sibling;
struct Missing;

macro_rules! test_scope {
    ($type:ty, $id:expr, $name:literal, $rank:expr) => {
        impl StaticScope for $type {
            const ID: ScopeId = $id;
            const RANK: u8 = $rank;
            const NAME: &'static str = $name;
        }
    };
}

test_scope!(Parent, PARENT_ID, "Parent", 100);
test_scope!(Child, CHILD_ID, "Child", 50);
test_scope!(Sibling, SIBLING_ID, "Sibling", 50);
test_scope!(Missing, MISSING_ID, "Missing", 25);

static PARENT: Parent = Parent;
static CHILD: Child = Child;
static SIBLING: Sibling = Sibling;
static MISSING: Missing = Missing;
static BOUNDARIES: [ScopeBoundary; 3] = [
    ScopeBoundary::new(&PARENT, ScopeParent::Root),
    ScopeBoundary::new(&CHILD, ScopeParent::Boundary(PARENT_ID)),
    ScopeBoundary::new(&SIBLING, ScopeParent::Root),
];

/// A root-scoped test component.
struct Root;
/// A factory-less seed declared at the parent boundary.
struct ParentSeed;
/// A factory-less seed declared at a sibling boundary.
struct SiblingSeed;
/// A factory-backed child component.
struct ChildFactory;
/// A factory-less seed assigned to an undeclared boundary.
struct MissingSeed;
/// First independent singleton in declaration order.
struct FirstSingleton;
/// Second independent singleton in declaration order.
struct SecondSingleton;
/// First independent component at a protocol-owned scope.
struct FirstScoped;
/// Second independent component at a protocol-owned scope.
struct SecondScoped;
/// Parent provider sharing a qualifier with a child provider.
struct ParentSharedProvider;
/// Child provider that must precede its qualified consumer.
struct ChildSharedProvider;
/// Child consumer of the duplicate cross-scope qualifier.
struct ChildSharedConsumer;
/// Singleton provider deliberately ordered behind a transient provider.
struct ScopedAlternativeProvider;
/// Transient provider selected first by canonical provider order.
struct SelectedTransientProvider;
/// Eager singleton dependency of the selected transient provider.
struct SelectedTransientDependency;
/// Consumer whose plan must include the selected transient's dependency.
struct SelectedTransientConsumer;

trait SharedProvider: Send + Sync {}
trait ReorderedProvider: Send + Sync {}

fn one<T: ?Sized + 'static>(name: &'static str, ty: &'static str) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(ty),
        Cardinality::One,
        ResolutionMode::Eager,
    )
}

fn shared<T: ?Sized + 'static>(name: &'static str, ty: &'static str) -> DependencyDescriptor {
    DependencyDescriptor {
        qualifier: Some("shared"),
        ..one::<T>(name, ty)
    }
}

fn empty_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("empty", Vec::new)()
}

fn parent_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("parent", || vec![one::<ParentSeed>("parent", "ParentSeed")])()
}

fn sibling_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("sibling", || vec![one::<SiblingSeed>(
        "sibling",
        "SiblingSeed"
    )])()
}

fn shared_provider_dependency() -> DependencyDescriptor {
    shared::<dyn SharedProvider>("shared", "SharedProvider")
}

fn shared_provider_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("shared-provider", || vec![shared_provider_dependency()])()
}

fn reordered_provider_dependency() -> DependencyDescriptor {
    shared::<dyn ReorderedProvider>("selected", "ReorderedProvider")
}

fn reordered_provider_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("reordered-provider", || vec![
        reordered_provider_dependency()
    ])()
}

fn transient_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("transient-dependency", || vec![one::<
        SelectedTransientDependency,
    >(
        "dependency",
        "SelectedTransientDependency"
    )])()
}

fn no_factory() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

fn descriptor<T: 'static>(
    id: &'static str,
    name: &'static str,
    scope: &'static dyn Scope,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor::manual(id, name, TypeDescriptor::of::<T>(name), scope)
        .with_factories(factories)
}

fn topology() -> PreparedScopeTopology {
    ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect("test topology prepares")
}

fn selection(descriptors: &[ComponentDescriptor]) -> ProviderSelectionModel {
    ComponentRegistry {
        components: descriptors.to_vec(),
        providers: Vec::new(),
    }
    .provider_selection_model(descriptors)
    .expect("test provider selection model validates")
}

fn provider<T: 'static, P: ?Sized + 'static>(
    qualifier: &'static str,
    priority: i64,
    ordering: &'static [ProviderOrder],
) -> ProviderDescriptor {
    planned_provider(
        TypeDescriptor::of::<P>("provider trait"),
        TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        qualifier,
        false,
    )
    .priority(priority)
    .ordering(ordering)
}

fn selection_with(
    descriptors: &[ComponentDescriptor],
    providers: Vec<ProviderDescriptor>,
) -> ProviderSelectionModel {
    ComponentRegistry {
        components: descriptors.to_vec(),
        providers,
    }
    .provider_selection_model(descriptors)
    .expect("test provider selection model validates")
}

mod partition;
mod selection;
