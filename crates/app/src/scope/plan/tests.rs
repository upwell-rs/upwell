use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, StaticScope, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentFactoryDescriptor, ComponentRegistry,
    DependencyTarget, ProviderDescriptor, ProviderOrder, ProviderOrderDirection,
    ProviderSelectionModel, Singleton,
};

use super::*;
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

fn construct(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { unreachable!("planning does not invoke factories") })
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn parent_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "parent",
        ty: TypeDescriptor::of::<ParentSeed>("ParentSeed"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn sibling_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "sibling",
        ty: TypeDescriptor::of::<SiblingSeed>("SiblingSeed"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn shared_provider_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "shared",
        ty: TypeDescriptor::of::<dyn SharedProvider>("SharedProvider"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: Some("shared"),
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn reordered_provider_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "selected",
        ty: TypeDescriptor::of::<dyn ReorderedProvider>("ReorderedProvider"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: Some("shared"),
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

fn transient_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "dependency",
        ty: TypeDescriptor::of::<SelectedTransientDependency>("SelectedTransientDependency"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: upwell_core::DependencyObservation::Snapshot,
    }]
}

static EMPTY_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "empty",
    construct,
    dependencies: no_dependencies,
    default: false,
}];
static PARENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "parent",
    construct,
    dependencies: parent_dependency,
    default: false,
}];
static SIBLING_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "sibling",
    construct,
    dependencies: sibling_dependency,
    default: false,
}];
static SHARED_PROVIDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "shared-provider",
    construct,
    dependencies: shared_provider_dependency,
    default: false,
}];
static REORDERED_PROVIDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "reordered-provider",
    construct,
    dependencies: reordered_provider_dependency,
    default: false,
}];
static TRANSIENT_DEPENDENCY_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "transient-dependency",
        construct,
        dependencies: transient_dependency,
        default: false,
    }];

fn empty_factory() -> &'static [ComponentFactoryDescriptor] {
    &EMPTY_FACTORY
}

fn parent_factory() -> &'static [ComponentFactoryDescriptor] {
    &PARENT_FACTORY
}

fn sibling_factory() -> &'static [ComponentFactoryDescriptor] {
    &SIBLING_FACTORY
}

fn shared_provider_factory() -> &'static [ComponentFactoryDescriptor] {
    &SHARED_PROVIDER_FACTORY
}

fn reordered_provider_factory() -> &'static [ComponentFactoryDescriptor] {
    &REORDERED_PROVIDER_FACTORY
}

fn transient_dependency_factory() -> &'static [ComponentFactoryDescriptor] {
    &TRANSIENT_DEPENDENCY_FACTORY
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
    ComponentDescriptor {
        id,
        name,
        ty: TypeDescriptor::of::<T>(name),
        scope,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
    }
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

fn erase(_: &BoxedComponent) -> BoxedComponent {
    panic!("planning tests do not erase provider instances")
}

fn provider<T: 'static, P: ?Sized + 'static>(
    qualifier: &'static str,
    priority: i64,
    ordering: &'static [ProviderOrder],
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<P>("provider trait"),
        concrete_ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        qualifier,
        primary: false,
        priority,
        ordering,
        erase,
    }
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

#[test]
fn factoryless_descriptor_in_undeclared_scope_is_rejected() {
    let descriptors = [descriptor::<MissingSeed>(
        "missing_seed",
        "MissingSeed",
        &MISSING,
        no_factory,
    )];

    let error = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect_err("factory-less descriptors require declared scopes");

    assert!(matches!(
        error,
        Error::UndeclaredScope {
            component,
            scope: MISSING_ID,
        } if component.ends_with("MissingSeed")
    ));
}

#[test]
fn partition_preserves_independent_singleton_declaration_order() {
    let descriptors = [
        descriptor::<SecondSingleton>("z-second", "Second", &Singleton, empty_factory),
        descriptor::<FirstSingleton>("a-first", "First", &Singleton, empty_factory),
    ];
    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.singletons[0].id, descriptors[0].id);
    assert_eq!(plan.singletons[1].id, descriptors[1].id);
}

#[test]
fn partition_preserves_independent_scoped_declaration_order() {
    let descriptors = [
        descriptor::<SecondScoped>("z-second", "Second", &CHILD, empty_factory),
        descriptor::<FirstScoped>("a-first", "First", &CHILD, empty_factory),
    ];
    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.orders[&CHILD_ID][0].id, descriptors[0].id);
    assert_eq!(plan.orders[&CHILD_ID][1].id, descriptors[1].id);
}

#[test]
fn child_order_accepts_root_ancestor_and_local_factoryless_descriptors() {
    let descriptors = [
        descriptor::<Root>("root", "Root", &Singleton, empty_factory),
        descriptor::<ParentSeed>("parent_seed", "ParentSeed", &PARENT, no_factory),
        descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, parent_factory),
    ];

    let plan = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect("plan succeeds");

    assert_eq!(plan.orders[&CHILD_ID].len(), 1);
    assert_eq!(
        plan.orders[&CHILD_ID][0].ty.type_id,
        TypeId::of::<ChildFactory>()
    );
    assert_eq!(
        plan.seed_destinations[&TypeId::of::<ParentSeed>()].scope,
        PARENT_ID
    );
}

#[test]
fn child_order_does_not_treat_sibling_factoryless_descriptors_as_prebuilt() {
    let descriptors = [
        descriptor::<SiblingSeed>("sibling_seed", "SiblingSeed", &SIBLING, no_factory),
        descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, sibling_factory),
    ];

    let error = ScopePlan::partition(&descriptors, &selection(&descriptors), &topology())
        .expect_err("sibling seed is not reachable while planning child");

    assert!(matches!(
        error,
        Error::Di(upwell_di::Error::DependencyCycle(_))
    ));
}

#[test]
fn topology_aware_registry_validation_rejects_sibling_dependencies() {
    let registry = crate::AppRegistry {
        components: vec![
            descriptor::<SiblingSeed>("sibling_seed", "SiblingSeed", &SIBLING, no_factory),
            descriptor::<ChildFactory>("child", "ChildFactory", &CHILD, sibling_factory),
        ],
        providers: Vec::new(),
        config_bindings: Vec::new(),
        condition_facts: Vec::new(),
    };

    let error = registry
        .validate_with_scope_topology(&topology())
        .expect_err("sibling dependency must be unreachable");

    assert!(matches!(
        error,
        Error::Di(upwell_di::Error::ScopeViolation(_))
    ));
}

#[test]
fn duplicate_qualifiers_across_scopes_plan_the_runtime_visible_provider() {
    let parent = descriptor::<ParentSharedProvider>(
        "parent-shared",
        "ParentSharedProvider",
        &PARENT,
        empty_factory,
    );
    let child = descriptor::<ChildSharedProvider>(
        "child-shared",
        "ChildSharedProvider",
        &CHILD,
        empty_factory,
    );
    let consumer = descriptor::<ChildSharedConsumer>(
        "child-consumer",
        "ChildSharedConsumer",
        &CHILD,
        shared_provider_factory,
    );
    let descriptors = [parent, consumer, child];
    let selection = selection_with(
        &descriptors,
        vec![
            provider::<ParentSharedProvider, dyn SharedProvider>("shared", 0, &[]),
            provider::<ChildSharedProvider, dyn SharedProvider>("shared", 0, &[]),
        ],
    );
    let selected = selection.selected_dependencies_with_scope_reachability(
        &consumer,
        &shared_provider_dependency()[0],
        |consumer, dependency| topology().is_reachable(&consumer, &dependency),
    );
    let plan = ScopePlan::partition(&descriptors, &selection, &topology())
        .expect("cross-scope duplicate qualifier plans");

    assert!(matches!(
        selected.as_slice(),
        [upwell_di::SelectedDependency {
            target: DependencyTarget::Provider(provider),
            ..
        }] if provider.concrete_ty.type_id == TypeId::of::<ChildSharedProvider>()
    ));
    assert_eq!(
        plan.orders[&CHILD_ID]
            .iter()
            .map(|component| component.id)
            .collect::<Vec<_>>(),
        ["child-shared", "child-consumer"]
    );
}

#[test]
fn reordered_transient_selection_plans_its_eager_dependencies() {
    static TRANSIENT_BEFORE_SCOPED: [ProviderOrder; 1] = [ProviderOrder {
        target: TypeDescriptor::of::<ScopedAlternativeProvider>("ScopedAlternativeProvider"),
        traits: &[],
        direction: ProviderOrderDirection::Before,
    }];
    let consumer = descriptor::<SelectedTransientConsumer>(
        "consumer",
        "SelectedTransientConsumer",
        &Singleton,
        reordered_provider_factory,
    );
    let dependency = descriptor::<SelectedTransientDependency>(
        "transient-dependency",
        "SelectedTransientDependency",
        &Singleton,
        empty_factory,
    );
    let scoped = descriptor::<ScopedAlternativeProvider>(
        "scoped-alternative",
        "ScopedAlternativeProvider",
        &Singleton,
        empty_factory,
    );
    let transient = descriptor::<SelectedTransientProvider>(
        "selected-transient",
        "SelectedTransientProvider",
        &upwell_core::Transient,
        transient_dependency_factory,
    );
    let descriptors = [consumer, scoped, dependency, transient];
    let selection = selection_with(
        &descriptors,
        vec![
            provider::<ScopedAlternativeProvider, dyn ReorderedProvider>("shared", 20, &[]),
            provider::<SelectedTransientProvider, dyn ReorderedProvider>(
                "shared",
                -20,
                &TRANSIENT_BEFORE_SCOPED,
            ),
        ],
    );
    let selected = selection.selected_dependencies_with_scope_reachability(
        &consumer,
        &reordered_provider_dependency()[0],
        |consumer, dependency| consumer == dependency,
    );
    let plan = ScopePlan::partition(&descriptors, &selection, &topology())
        .expect("selected transient dependency plans");
    let order = plan
        .singletons
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let planned = topological_sort(
        &plan.singletons,
        &HashSet::new(),
        &selection,
        |consumer, dependency| consumer == dependency,
    )
    .expect("root construction order plans")
    .into_iter()
    .map(|component| component.id)
    .collect::<Vec<_>>();

    assert!(matches!(
        selected.as_slice(),
        [upwell_di::SelectedDependency {
            target: DependencyTarget::Provider(provider),
            ..
        }] if provider.concrete_ty.type_id == TypeId::of::<SelectedTransientProvider>()
    ));
    assert_eq!(
        order,
        ["consumer", "scoped-alternative", "transient-dependency"]
    );
    assert!(
        planned.iter().position(|id| *id == "transient-dependency")
            < planned.iter().position(|id| *id == "consumer")
    );
}
