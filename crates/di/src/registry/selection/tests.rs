use std::future::Future;
use std::pin::Pin;

use upwell_core::{DependencyDescriptor, Singleton, StaticScope, Transient, TypeDescriptor};

use super::*;
use crate::descriptors::{
    BoxedComponent, ComponentConstructionContext, ComponentFactoryDescriptor,
};

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

struct Consumer;
struct Near;
struct Far;
struct TransientProvider;
struct Manual;

fn fake_factory<'a>(
    _: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>> {
    Box::pin(async { unreachable!("selection tests do not construct components") })
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

static FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fake_factory,
    dependencies: no_dependencies,
    default: false,
}];

fn factory() -> &'static [ComponentFactoryDescriptor] {
    &FACTORY
}

fn manual() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

fn component<T: 'static>(
    id: &'static str,
    scope: &'static dyn Scope,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name: id,
        ty: TypeDescriptor::of::<T>(id),
        scope,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn provider<T: 'static>(
    qualifier: &'static str,
    primary: bool,
    priority: i64,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("SelectedTrait"),
        concrete_ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        qualifier,
        primary,
        priority,
        ordering: &[],
        erase: erase_unreachable,
    }
}

fn erase_unreachable(_: &BoxedComponent) -> BoxedComponent {
    unreachable!("selection tests do not erase providers")
}

fn dependency(
    cardinality: Cardinality,
    resolution: ResolutionMode,
    qualifier: Option<&'static str>,
) -> DependencyDescriptor {
    DependencyDescriptor {
        name: "SelectedTrait",
        ty: TypeDescriptor::of::<dyn Send>("SelectedTrait"),
        cardinality,
        optional: false,
        dynamic: false,
        qualifier,
        config: false,
        resolution,
        observation: upwell_core::DependencyObservation::Snapshot,
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

#[test]
fn eager_single_matches_global_transient_then_built_then_transient_fallback() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let transient = component::<TransientProvider>("transient", &Transient, factory);
    let components = vec![consumer, near, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>("transient", false, 0),
            provider::<Near>("near", true, 0),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("dependency selection validates");

    assert_eq!(selected_types(&selected), [TypeId::of::<Near>()]);
    assert_eq!(
        selected[0].reason,
        DependencySelectionReason::SoleProviderInWinningSet
    );
    assert_eq!(selected[0].scope, Some(NEAR_ID));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::ScopePrecedence)
    );

    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>("transient", true, 0),
            provider::<Near>("near", false, 0),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("dependency selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<TransientProvider>()]
    );
    assert_eq!(
        selected[0].reason,
        DependencySelectionReason::PrimaryProviderInWinningSet
    );
    assert_eq!(selected[0].scope, Some(Transient.id()));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::TransientPrecedence)
    );
}

#[test]
fn qualified_and_deferred_selection_preserve_runtime_transient_semantics() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let transient = component::<TransientProvider>("transient", &Transient, factory);
    let components = vec![consumer, near, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>("shared", false, -10),
            provider::<Near>("shared", false, 0),
        ],
    };

    let eager = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Eager, Some("shared")),
            &components,
            reaches,
        )
        .expect("eager dependency selection validates");
    let deferred = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Deferred, Some("shared")),
            &components,
            reaches,
        )
        .expect("deferred dependency selection validates");

    assert_eq!(selected_types(&eager), [TypeId::of::<TransientProvider>()]);
    assert_eq!(selected_types(&deferred), [TypeId::of::<Near>()]);
    assert_eq!(deferred[0].reason, DependencySelectionReason::Qualified);
    assert_eq!(deferred[0].scope, Some(NEAR_ID));
    assert_eq!(
        deferred[0].stage,
        Some(DependencySelectionStage::ScopePrecedence)
    );
}

#[test]
fn fresh_selection_reports_accessible_factoryless_providers() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let far = component::<Far>("far", &FarScope, factory);
    let manual = component::<Manual>("manual", &NearScope, manual);
    let components = vec![consumer, near, far, manual];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Manual>("manual", true, 0),
            provider::<Far>("far", false, 1),
            provider::<Near>("near", false, 2),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Collection, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh dependency selection validates");

    assert_eq!(
        selected_types(&selected),
        [
            TypeId::of::<Manual>(),
            TypeId::of::<Far>(),
            TypeId::of::<Near>()
        ]
    );
}

#[test]
fn fresh_keyed_selection_reports_accessible_factoryless_providers() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let manual = component::<Manual>("manual", &NearScope, manual);
    let components = vec![consumer, near, manual];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Manual>("manual", false, 0),
            provider::<Near>("near", false, 1),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<Manual>(), TypeId::of::<Near>()]
    );
}

#[test]
fn fresh_qualified_and_keyed_selection_match_ordered_runtime_precedence() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let far = component::<Far>("far", &FarScope, factory);
    let components = vec![consumer, near, far];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>("shared", false, 20),
            provider::<Far>("shared", false, -10),
        ],
    };
    let qualified = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Fresh, Some("shared")),
            &components,
            reaches,
        )
        .expect("fresh qualified selection validates");
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(selected_types(&qualified), [TypeId::of::<Near>()]);
    assert_eq!(qualified[0].scope, Some(NEAR_ID));
    assert_eq!(
        qualified[0].stage,
        Some(DependencySelectionStage::FreshScopePrecedence)
    );
    assert_eq!(selected_types(&keyed), [TypeId::of::<Near>()]);
    assert_eq!(keyed[0].scope, Some(NEAR_ID));
    assert_eq!(
        keyed[0].stage,
        Some(DependencySelectionStage::KeyedPrecedence)
    );
}

#[test]
fn fresh_keyed_collisions_prefer_nearer_scope_over_later_global_order() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let far = component::<Far>("far", &FarScope, factory);
    let components = vec![consumer, near, far];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>("shared", false, -10),
            provider::<Far>("shared", false, 20),
        ],
    };
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Keyed, ResolutionMode::Fresh, None),
            &components,
            reaches,
        )
        .expect("fresh keyed selection validates");

    assert_eq!(selected_types(&keyed), [TypeId::of::<Near>()]);
    assert_eq!(keyed[0].scope, Some(NEAR_ID));
}

#[test]
fn transient_fallback_uses_the_same_ordered_qualified_sets_as_runtime() {
    let consumer = component::<Consumer>("consumer", &Transient, factory);
    let inaccessible = component::<Near>("inaccessible", &NearScope, factory);
    let transient = component::<TransientProvider>("transient", &Transient, factory);
    let components = vec![consumer, inaccessible, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<TransientProvider>("shared", false, 20),
            provider::<Near>("shared", false, -10),
        ],
    };
    let selected = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::One, ResolutionMode::Eager, Some("shared")),
            &components,
            reaches,
        )
        .expect("transient fallback selection validates");

    assert_eq!(
        selected_types(&selected),
        [TypeId::of::<TransientProvider>()]
    );
    assert_eq!(selected[0].scope, Some(Transient.id()));
    assert_eq!(
        selected[0].stage,
        Some(DependencySelectionStage::TransientFallback)
    );
}

#[test]
fn collections_use_final_provider_order_and_keyed_collisions_use_runtime_precedence() {
    let consumer = component::<Consumer>("consumer", &NearScope, factory);
    let near = component::<Near>("near", &NearScope, factory);
    let far = component::<Far>("far", &FarScope, factory);
    let transient = component::<TransientProvider>("transient", &Transient, factory);
    let components = vec![consumer, near, far, transient];
    let registry = ComponentRegistry {
        components: components.clone(),
        providers: vec![
            provider::<Near>("shared", false, 20),
            provider::<TransientProvider>("shared", false, 10),
            provider::<Far>("shared", false, 0),
        ],
    };

    let collection = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Collection, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("collection selection validates");
    let keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Keyed, ResolutionMode::Eager, None),
            &components,
            reaches,
        )
        .expect("keyed selection validates");
    let deferred_keyed = registry
        .selected_dependencies_with_scope_reachability(
            &consumer,
            &dependency(Cardinality::Keyed, ResolutionMode::Deferred, None),
            &components,
            reaches,
        )
        .expect("deferred keyed selection validates");

    assert_eq!(
        selected_types(&collection),
        [
            TypeId::of::<Far>(),
            TypeId::of::<TransientProvider>(),
            TypeId::of::<Near>(),
        ]
    );
    assert_eq!(selected_types(&keyed), [TypeId::of::<TransientProvider>()]);
    assert_eq!(selected_types(&deferred_keyed), [TypeId::of::<Near>()]);
}

#[test]
fn direct_selection_model_construction_rejects_orphan_provider() {
    let orphan = provider::<Near>("orphan", false, 0);
    let error = match ProviderSelectionModel::new(&[], vec![orphan], HashMap::new()) {
        Ok(_) => panic!("orphan provider must be rejected"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::ProviderComponentMissing(_)));
}
