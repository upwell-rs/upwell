use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, ResolverSet, ScopeId, StaticScope,
    Transient, TypeDescriptor,
};

use crate::descriptors::component::from_boxed;
use crate::{ComponentFactoryDescriptor, Injectable};

use super::*;
use crate::registry::selection::select_single_provider;

/// A throwaway intermediate scope for exercising child-container construction
/// without depending on any protocol's concrete scopes.
struct TestScope;
struct SameNameScope;

impl Scope for TestScope {
    fn id(&self) -> ScopeId {
        ScopeId::new("test/container").expect("valid test scope ID")
    }

    fn rank(&self) -> u8 {
        1
    }

    fn name(&self) -> &'static str {
        "Test"
    }
}

impl Scope for SameNameScope {
    fn id(&self) -> ScopeId {
        ScopeId::new("test/same-name").expect("valid test scope ID")
    }

    fn rank(&self) -> u8 {
        1
    }

    fn name(&self) -> &'static str {
        "Test"
    }
}

const FRESH_TARGET_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-target");
const FRESH_OWNER_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-owner");

struct FreshTargetScope;

impl StaticScope for FreshTargetScope {
    const ID: ScopeId = FRESH_TARGET_SCOPE_ID;
    const RANK: u8 = 2;
    const NAME: &'static str = "FreshTarget";
}

struct FreshOwnerScope;

impl StaticScope for FreshOwnerScope {
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

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
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
    dependencies: no_dependencies,
    default: false,
}];
static OWNER_PROVIDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: owner_provider_factory,
    dependencies: no_dependencies,
    default: false,
}];
static FRESH_TARGET_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fresh_target_factory,
    dependencies: fresh_target_dependencies,
    default: false,
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

#[tokio::test]
async fn empty_child_scope_retains_its_identity() {
    let root = root().await;
    let registry = registry();

    let child = ScopeContainer::open_child(
        &TestScope,
        Arc::clone(&root),
        Arc::clone(&registry),
        &[],
        Vec::new(),
    )
    .await
    .expect("open child");

    assert!(!Arc::ptr_eq(&root, &child));
    assert_eq!(child.scope().id(), TestScope.id());
    assert!(child.belongs_to_registry(&registry));
    assert!(!child.belongs_to_registry(&self::registry()));
    assert!(!child.can_access(&SameNameScope));
    assert!(matches!(
        child.slot.state,
        ScopeResolverSlotState::Attached(_)
    ));
    assert!(Arc::ptr_eq(
        &child,
        &child.slot.resolve().expect("attached scope resolves")
    ));

    let source = child
        .resolvers()
        .get_arc::<ComponentSource>()
        .expect("empty child exposes its component source");

    assert!(Arc::ptr_eq(
        &child,
        &source.container.upgrade().expect("child remains alive")
    ));
}

#[tokio::test]
async fn child_scope_with_a_seed_is_built() {
    let root = root().await;

    let seed = BoxedComponent {
        ty: TypeDescriptor::of::<u8>("u8"),
        value: Box::new(7u8),
    };

    let child =
        ScopeContainer::open_child(&TestScope, Arc::clone(&root), registry(), &[], vec![seed])
            .await
            .expect("open child");

    assert!(
        !Arc::ptr_eq(&root, &child),
        "a seeded scope should allocate its own container"
    );
    assert_eq!(child.scope().name(), "Test");
}

static CONCRETE_ID_CALLS: AtomicUsize = AtomicUsize::new(0);

fn counted_concrete_id() -> TypeId {
    CONCRETE_ID_CALLS.fetch_add(1, Ordering::Relaxed);

    TypeId::of::<u8>()
}

fn erase_unreachable(_: &BoxedComponent) -> BoxedComponent {
    panic!("the provider index test never instantiates a provider")
}

#[test]
fn provider_lookup_uses_the_prebuilt_concrete_index() {
    const PROVIDERS: usize = 256;

    let concrete_ty = TypeDescriptor {
        name: "Counted",
        type_name: std::any::type_name::<u8>,
        type_id: counted_concrete_id(),
    };
    let provider = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty,
        qualifier: "counted",
        primary: false,
        priority: 0,
        ordering: &[],
        erase: erase_unreachable,
    };
    let component = ComponentDescriptor::manual("counted", "Counted", concrete_ty, &Singleton);
    let registry = ScopeRegistry::new(
        HashMap::new(),
        HashMap::from([(component.ty.type_id, component)]),
        vec![provider; PROVIDERS],
        HashMap::new(),
    )
    .expect("provider component is registered");

    CONCRETE_ID_CALLS.store(0, Ordering::SeqCst);

    for _ in 0..1_000 {
        assert_eq!(registry.providers_for(TypeId::of::<u8>()).len(), PROVIDERS);
    }

    assert_eq!(
        CONCRETE_ID_CALLS.load(Ordering::SeqCst),
        0,
        "provider lookup rescanned concrete descriptor functions"
    );
}

fn test_provider(name: &'static str, qualifier: &'static str, primary: bool) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty: TypeDescriptor::of::<u8>(name),
        qualifier,
        primary,
        priority: 0,
        ordering: &[],
        erase: erase_unreachable,
    }
}

#[test]
fn single_provider_selection_is_shared_and_ambiguity_aware() {
    let sole = [test_provider("Sole", "sole", false)];

    assert_eq!(
        select_single_provider(&sole).map(|p| p.qualifier),
        Some("sole")
    );

    let two_plain = [
        test_provider("First", "first", false),
        test_provider("Second", "second", false),
    ];

    assert!(select_single_provider(&two_plain).is_none());

    let unique_primary = [
        test_provider("First", "first", true),
        test_provider("Second", "second", false),
    ];

    assert_eq!(
        select_single_provider(&unique_primary).map(|p| p.qualifier),
        Some("first")
    );

    let two_primaries = [
        test_provider("First", "first", true),
        test_provider("Second", "second", true),
    ];

    assert!(select_single_provider(&two_primaries).is_none());
}

#[test]
fn registry_construction_completes_missing_provider_ordinals() {
    let provider = test_provider("Unplanned", "unplanned", false);
    let component =
        ComponentDescriptor::manual("unplanned", "Unplanned", provider.concrete_ty, &Singleton);
    let registry = ScopeRegistry::new(
        HashMap::new(),
        HashMap::from([(component.ty.type_id, component)]),
        vec![provider],
        HashMap::new(),
    )
    .expect("provider component is registered");

    // Previously this expect() panicked for providers missing from the plan.
    assert_eq!(registry.provider_ordinal(&provider), 0);
}

#[test]
fn registry_indexes_follow_final_provider_order_for_first_wins_lookups() {
    let first = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty: TypeDescriptor::of::<u8>("First"),
        qualifier: "shared",
        primary: false,
        priority: 20,
        ordering: &[],
        erase: erase_unreachable,
    };
    let winning = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty: TypeDescriptor::of::<u16>("Winning"),
        qualifier: "shared",
        primary: false,
        priority: -10,
        ordering: &[],
        erase: erase_unreachable,
    };
    let order = HashMap::from([(
        TypeId::of::<dyn Send>(),
        HashMap::from([(TypeId::of::<u8>(), 1), (TypeId::of::<u16>(), 0)]),
    )]);
    let components = [first, winning]
        .into_iter()
        .map(|provider| {
            let component = ComponentDescriptor::manual(
                provider.qualifier,
                provider.concrete_ty.name,
                provider.concrete_ty,
                &Singleton,
            );

            (component.ty.type_id, component)
        })
        .collect();
    let registry = ScopeRegistry::new(HashMap::new(), components, vec![first, winning], order)
        .expect("provider components are registered");

    assert_eq!(
        registry
            .selection
            .select_global(TypeId::of::<dyn Send>(), Some("shared"))
            .map(|provider| provider.concrete_ty.type_id),
        Some(TypeId::of::<u16>())
    );
    assert_eq!(
        registry
            .selection
            .providers_for_trait(TypeId::of::<dyn Send>())
            .iter()
            .map(|provider| provider.concrete_ty.type_id)
            .collect::<Vec<_>>(),
        [TypeId::of::<u16>(), TypeId::of::<u8>()]
    );
}

#[test]
fn registry_construction_rejects_orphan_provider() {
    let provider = test_provider("Orphan", "orphan", false);
    let error = match ScopeRegistry::new(
        HashMap::new(),
        HashMap::new(),
        vec![provider],
        HashMap::new(),
    ) {
        Ok(_) => panic!("orphan provider must be rejected"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::ProviderComponentMissing(_)));
}

#[tokio::test]
async fn fresh_target_resolves_from_its_declared_scope_ancestry() {
    let target_provider = ComponentDescriptor {
        id: "target-provider",
        name: "TargetProvider",
        ty: TypeDescriptor::of::<TargetProvider>("TargetProvider"),
        scope: &FreshTargetScope,
        condition: None,
        factories: target_provider_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let owner_provider = ComponentDescriptor {
        id: "owner-provider",
        name: "OwnerProvider",
        ty: TypeDescriptor::of::<OwnerProvider>("OwnerProvider"),
        scope: &FreshOwnerScope,
        condition: None,
        factories: owner_provider_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let fresh_target = ComponentDescriptor {
        id: "fresh-target",
        name: "FreshTarget",
        ty: TypeDescriptor::of::<FreshTarget>("FreshTarget"),
        scope: &FreshTargetScope,
        condition: None,
        factories: fresh_target_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let target_seed = ComponentDescriptor::manual(
        "target-seed",
        "TargetSeed",
        TypeDescriptor::of::<TargetSeed>("TargetSeed"),
        &FreshTargetScope,
    );
    let components = [target_provider, owner_provider, fresh_target, target_seed]
        .into_iter()
        .map(|descriptor| (descriptor.ty.type_id, descriptor))
        .collect();
    let providers = vec![
        provider_descriptor(target_provider.ty, erase_target_provider),
        provider_descriptor(owner_provider.ty, erase_owner_provider),
    ];
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");
    let seed = boxed_component("TargetSeed", Arc::new(TargetSeed("target-seed")));
    let target_scope = ScopeContainer::open_child(
        &FreshTargetScope,
        root,
        Arc::clone(&registry),
        &[target_seed, target_provider],
        vec![seed],
    )
    .await
    .expect("target scope opens");
    let owner_scope = ScopeContainer::open_child(
        &FreshOwnerScope,
        target_scope,
        Arc::clone(&registry),
        &[owner_provider],
        Vec::new(),
    )
    .await
    .expect("owner scope opens");

    let boxed = construct_fresh_boxed(&registry, owner_scope, fresh_target)
        .await
        .expect("fresh target constructs");
    let target = from_boxed::<Arc<FreshTarget>>(&boxed).expect("fresh target stored");

    assert_eq!(
        target.provider.source(),
        "target",
        "the owner's shorter-lived provider must not leak into fresh construction"
    );
    assert_eq!(
        target.seed.0, "target-seed",
        "a dynamic seed in the target's declared scope remains accessible"
    );
}

#[tokio::test]
async fn fresh_reconstructs_a_transient_against_the_requesting_scope() {
    struct RootBound;

    #[derive(Clone)]
    struct TransientLeaf {
        bound: Arc<RootBound>,
    }

    impl Injectable for TransientLeaf {
        type Target = Self;
        type Stored = Self;

        fn into_stored(self) -> Self {
            self
        }

        fn from_stored(stored: &Self) -> Self {
            stored.clone()
        }
    }

    fn transient_leaf_factory(
        cx: &mut ComponentConstructionContext,
    ) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
        Box::pin(async {
            let bound = cx
                .resolve::<Arc<RootBound>>()
                .await?
                .ok_or(Error::MissingComponent("RootBound"))?;

            Ok(BoxedComponent {
                ty: TypeDescriptor::of::<TransientLeaf>("TransientLeaf"),
                value: Box::new(Injectable::into_stored(TransientLeaf { bound })),
            })
        })
    }

    static TRANSIENT_LEAF_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: transient_leaf_factory,
        dependencies: no_dependencies,
        default: false,
    }];

    let bound = ComponentDescriptor::manual(
        "root-bound",
        "RootBound",
        TypeDescriptor::of::<RootBound>("RootBound"),
        &Singleton,
    );
    let transient_leaf = ComponentDescriptor {
        id: "transient-leaf",
        name: "TransientLeaf",
        ty: TypeDescriptor::of::<TransientLeaf>("TransientLeaf"),
        scope: &Transient,
        condition: None,
        factories: || &TRANSIENT_LEAF_FACTORY,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let components = [(bound.ty.type_id, bound)].into_iter().collect();
    let transients = [(transient_leaf.ty.type_id, transient_leaf)]
        .into_iter()
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(transients, components, Vec::new(), HashMap::new())
            .expect("registry validates"),
    );
    let bound = Arc::new(RootBound);
    let root = ScopeContainer::build_root(
        &[],
        vec![boxed_component("RootBound", Arc::clone(&bound))],
        ResolverSet::new(),
        Arc::clone(&registry),
    )
    .await
    .expect("root builds");

    let boxed = construct_fresh_boxed(&registry, root, transient_leaf)
        .await
        .expect("a factory-backed transient reconstructs against the requesting scope");
    let leaf = from_boxed::<TransientLeaf>(&boxed).expect("transient leaf stored");

    assert!(
        Arc::ptr_eq(&leaf.bound, &bound),
        "the rebuilt transient resolves its dependency up the requesting scope's chain"
    );
}

#[tokio::test]
async fn component_source_does_not_keep_a_scope_alive() {
    let root = root().await;
    let source = root
        .resolvers()
        .get_arc::<ComponentSource>()
        .expect("component source installed");

    drop(root);

    assert!(
        source.component::<NeverRegistered>().is_none(),
        "a weak component source must not retain its container"
    );
}

struct NeverRegistered;

impl Component for NeverRegistered {
    type Handle = Arc<Self>;

    const ID: &'static str = "never-registered";
    const NAME: &'static str = "NeverRegistered";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}
