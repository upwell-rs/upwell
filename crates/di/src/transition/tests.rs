use std::future::Future;
use std::pin::Pin;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, RuntimeGenerationId,
    TypeDescriptor,
};

use super::*;
use crate::{
    BoxedComponent, ComponentConstructionContext, ComponentFactoryDescriptor, ProviderDescriptor,
    ProviderOrder, Singleton,
};

struct DefaultAuthenticator;
struct CustomAuthenticator;
struct LiveConsumer;
struct FixedConsumer;
struct FixedDependent;
struct Unrelated;
struct RequestScope;

trait Authenticator: Send + Sync {}

impl upwell_core::StaticScope for RequestScope {
    const ID: upwell_core::ScopeId =
        upwell_core::namespaced_id!(upwell_core::ScopeId, "test/request");
    const RANK: u8 = 1;
    const NAME: &'static str = "Request";
}

fn panic_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    panic!("transition planning must not construct components")
}

fn changed_panic_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    panic!("changed transition recipe must not construct components")
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn live_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<dyn Authenticator>(DependencyObservation::Live)]
}

fn fixed_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<dyn Authenticator>(
        DependencyObservation::Snapshot,
    )]
}

fn dependent_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<FixedConsumer>(DependencyObservation::Snapshot)]
}

fn dependency<T: ?Sized + 'static>(observation: DependencyObservation) -> DependencyDescriptor {
    DependencyDescriptor {
        name: std::any::type_name::<T>(),
        ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation,
    }
}

static EMPTY_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "empty",
    construct: panic_factory,
    dependencies: no_dependencies,
    default: false,
}];
static LIVE_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "live",
    construct: panic_factory,
    dependencies: live_dependencies,
    default: false,
}];
static FIXED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "fixed",
    construct: panic_factory,
    dependencies: fixed_dependencies,
    default: false,
}];
static DEPENDENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "dependent",
    construct: panic_factory,
    dependencies: dependent_dependencies,
    default: false,
}];
static CHANGED_EMPTY_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "changed-empty",
    construct: changed_panic_factory,
    dependencies: no_dependencies,
    default: false,
}];

fn empty_factory() -> &'static [ComponentFactoryDescriptor] {
    &EMPTY_FACTORY
}

fn live_factory() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_FACTORY
}

fn fixed_factory() -> &'static [ComponentFactoryDescriptor] {
    &FIXED_FACTORY
}

fn dependent_factory() -> &'static [ComponentFactoryDescriptor] {
    &DEPENDENT_FACTORY
}

fn changed_empty_factory() -> &'static [ComponentFactoryDescriptor] {
    &CHANGED_EMPTY_FACTORY
}

fn component<T: 'static>(
    id: &'static str,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name: id,
        ty: TypeDescriptor::of::<T>(id),
        scope: &Singleton,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn provider<T: 'static>(qualifier: &'static str, primary: bool) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
        concrete_ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        qualifier,
        primary,
        priority: 0,
        ordering: &[] as &[ProviderOrder],
        erase: |_| panic!("transition planning must not erase components"),
    }
}

fn registry(custom: bool) -> ComponentRegistry {
    let mut components = vec![
        component::<DefaultAuthenticator>("default-authenticator", empty_factory),
        component::<LiveConsumer>("live-consumer", live_factory),
        component::<FixedConsumer>("fixed-consumer", fixed_factory),
        component::<FixedDependent>("fixed-dependent", dependent_factory),
        component::<Unrelated>("unrelated", empty_factory),
    ];
    let mut providers = vec![provider::<DefaultAuthenticator>("default", !custom)];

    if custom {
        components.insert(
            1,
            component::<CustomAuthenticator>("custom-authenticator", empty_factory),
        );
        providers.push(provider::<CustomAuthenticator>("custom", true));
    }

    ComponentRegistry {
        components,
        providers,
    }
}

fn graph(custom: bool) -> EffectiveGraph {
    EffectiveGraph::build(RuntimeGenerationId::INITIAL, &registry(custom), |_, _| true)
        .expect("fixture graph validates")
}

#[test]
fn provider_switch_retains_live_consumer_and_replaces_fixed_closure() {
    let active = graph(false);
    let candidate = graph(true);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "custom-authenticator"), Some(NodeAction::Add));
    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::RebindLive));
    assert_eq!(action(&plan, "fixed-consumer"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "fixed-dependent"), Some(NodeAction::Replace));
    assert_eq!(action(&plan, "unrelated"), None);
    assert_eq!(plan.bindings.len(), 1);
}

#[test]
fn repeated_planning_is_deterministic() {
    let active = graph(false);
    let candidate = graph(true);

    assert_eq!(
        active.plan_transition(&candidate),
        active.plan_transition(&candidate)
    );
}

#[test]
fn identical_graphs_produce_a_no_op_plan() {
    let active = graph(false);
    let candidate = graph(false);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert!(plan.is_noop());
    assert!(
        plan.diff
            .nodes
            .iter()
            .all(|change| change.kinds.as_ref() == [NodeChangeKind::Unchanged])
    );
}

#[test]
fn replacement_suppresses_redundant_live_binding_work() {
    let active = graph(false);
    let mut candidate_registry = registry(true);
    let live = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "live-consumer")
        .expect("live consumer exists");
    live.factories = fixed_factory;
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::INITIAL, &candidate_registry, |_, _| {
            true
        })
        .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "live-consumer"), Some(NodeAction::Replace));
    assert!(
        plan.bindings
            .iter()
            .all(|binding| binding.dependency.consumer != "live-consumer")
    );
}

#[test]
fn retirement_orders_fixed_consumers_before_dependencies() {
    let active = graph(true);
    let candidate = graph(false);

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");
    let fixed = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-consumer")
        .expect("fixed consumer retires");
    let dependent = plan
        .retirement_order
        .iter()
        .position(|component| *component == "fixed-dependent")
        .expect("fixed dependent retires");

    assert!(dependent < fixed);
}

#[test]
fn stale_candidate_generation_is_rejected() {
    let active = graph(false);
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::new(1), &registry(true), |_, _| true)
            .expect("candidate validates");

    assert_eq!(
        active.plan_transition(&candidate),
        Err(StaleGraphCandidate {
            active: RuntimeGenerationId::INITIAL,
            candidate: RuntimeGenerationId::new(1),
        })
    );
}

#[test]
fn singleton_to_scoped_change_still_retires_the_active_singleton() {
    let active = graph(false);
    let mut candidate_registry = registry(false);
    let unrelated = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "unrelated")
        .expect("unrelated component exists");
    unrelated.scope = &RequestScope;
    let candidate = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        |consumer, dependency| consumer == dependency,
    )
    .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert!(plan.retirement_order.contains(&"unrelated"));
    assert!(!plan.construction_order.contains(&"unrelated"));
}

#[test]
fn changed_recipe_identity_replaces_the_component() {
    let active = graph(false);
    let mut candidate_registry = registry(false);
    let unrelated = candidate_registry
        .components
        .iter_mut()
        .find(|component| component.id == "unrelated")
        .expect("unrelated component exists");
    unrelated.factories = changed_empty_factory;
    let candidate =
        EffectiveGraph::build(RuntimeGenerationId::INITIAL, &candidate_registry, |_, _| {
            true
        })
        .expect("candidate validates");

    let plan = active
        .plan_transition(&candidate)
        .expect("candidate uses active base generation");

    assert_eq!(action(&plan, "unrelated"), Some(NodeAction::Replace));
    assert!(plan.diff.nodes.iter().any(|change| {
        change.component == "unrelated" && change.kinds.contains(&NodeChangeKind::FactoryChanged)
    }));
}

fn action(plan: &TransitionPlan, component: &str) -> Option<NodeAction> {
    plan.nodes
        .iter()
        .find(|node| node.component == component)
        .map(|node| node.action)
}
