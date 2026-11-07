use std::collections::BTreeSet;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, RuntimeGenerationId,
    TypeDescriptor,
};

use super::*;
use crate::test_support::{component, no_dependencies, provider, unbuilt_factories};
use crate::{ComponentFactoryDescriptor, ComponentRegistry};

struct DefaultAuthenticator;
struct CustomAuthenticator;
struct LiveConsumer;
struct LiveConsumerDependent;
struct LiveChainConsumer;
struct LiveChainDependent;
struct FixedConsumer;
struct FixedDependent;
struct Unrelated;
struct RequestScope;

trait Authenticator: Send + Sync {}

const AUTHENTICATOR: TypeDescriptor = TypeDescriptor::of::<dyn Authenticator>("Authenticator");

impl upwell_core::Scope for RequestScope {
    const ID: upwell_core::ScopeId =
        upwell_core::namespaced_id!(upwell_core::ScopeId, "test/request");
    const RANK: u8 = 1;
    const NAME: &'static str = "Request";
}

fn dependency<T: ?Sized + 'static>(observation: DependencyObservation) -> DependencyDescriptor {
    let name = std::any::type_name::<T>();

    DependencyDescriptor {
        observation,
        ..crate::test_support::dependency(
            name,
            TypeDescriptor::of::<T>(name),
            Cardinality::One,
            ResolutionMode::Eager,
        )
    }
}

fn empty_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("empty", no_dependencies)()
}

fn live_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("live", || vec![dependency::<dyn Authenticator>(
        DependencyObservation::Live
    )])()
}

fn fixed_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("fixed", || vec![dependency::<dyn Authenticator>(
        DependencyObservation::Snapshot
    )])()
}

fn dependent_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("dependent", || vec![dependency::<FixedConsumer>(
        DependencyObservation::Snapshot
    )])()
}

fn live_consumer_dependent_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("live-consumer-dependent", || vec![
        dependency::<LiveConsumer>(DependencyObservation::Snapshot)
    ])()
}

fn live_chain_consumer_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("live-chain-consumer", || vec![dependency::<
        LiveConsumerDependent,
    >(
        DependencyObservation::Live
    )])()
}

fn live_chain_dependent_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("live-chain-dependent", || vec![dependency::<
        LiveChainConsumer,
    >(
        DependencyObservation::Snapshot
    )])()
}

fn changed_empty_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("changed-empty", no_dependencies)()
}

fn registry(custom: bool) -> ComponentRegistry {
    let mut components = vec![
        component::<DefaultAuthenticator>("default-authenticator", empty_factory),
        component::<LiveConsumer>("live-consumer", live_factory),
        component::<LiveConsumerDependent>(
            "live-consumer-dependent",
            live_consumer_dependent_factory,
        ),
        component::<LiveChainConsumer>("live-chain-consumer", live_chain_consumer_factory),
        component::<LiveChainDependent>("live-chain-dependent", live_chain_dependent_factory),
        component::<FixedConsumer>("fixed-consumer", fixed_factory),
        component::<FixedDependent>("fixed-dependent", dependent_factory),
        component::<Unrelated>("unrelated", empty_factory),
    ];
    let mut providers = vec![provider::<DefaultAuthenticator>(
        AUTHENTICATOR,
        "default",
        !custom,
    )];

    if custom {
        components.insert(
            1,
            component::<CustomAuthenticator>("custom-authenticator", empty_factory),
        );
        providers.push(provider::<CustomAuthenticator>(
            AUTHENTICATOR,
            "custom",
            true,
        ));
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

fn action(plan: &TransitionPlan, component: &str) -> Option<NodeAction> {
    plan.nodes
        .iter()
        .find(|node| node.component == component)
        .map(|node| node.action)
}

mod invalidation;
mod planning;
mod rebinding;
