use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, TypeDescriptor};

use super::*;
use crate::test_support::{component, test_component};
use crate::{ComponentFactoryDescriptor, ScopeContainer, ScopeRegistry, Singleton, factories};

struct CandidateDependency(&'static str);

struct CandidateConsumer {
    dependency: Arc<CandidateDependency>,
}

struct WrongCandidate;

test_component!(CandidateDependency, "candidate-dependency");
test_component!(CandidateConsumer, "candidate-consumer");
test_component!(WrongCandidate, "wrong-candidate");

async fn candidate_dependency() -> CandidateDependency {
    CandidateDependency("candidate")
}

async fn candidate_consumer(dependency: Arc<CandidateDependency>) -> CandidateConsumer {
    CandidateConsumer { dependency }
}

async fn wrong_candidate() -> WrongCandidate {
    WrongCandidate
}

async fn panicking_candidate() -> CandidateDependency {
    panic!("candidate canary")
}

fn candidate_dependency_factories() -> &'static [ComponentFactoryDescriptor] {
    factories!["candidate" => candidate_dependency]()
}

fn candidate_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    factories!["candidate" => candidate_consumer]()
}

fn wrong_candidate_factories() -> &'static [ComponentFactoryDescriptor] {
    factories!["wrong" => wrong_candidate]()
}

fn panicking_candidate_factories() -> &'static [ComponentFactoryDescriptor] {
    factories!["panicking" => panicking_candidate]()
}

fn registry(descriptors: impl IntoIterator<Item = ComponentDescriptor>) -> Arc<ScopeRegistry> {
    let components = descriptors
        .into_iter()
        .map(|descriptor| (descriptor.ty.type_id, descriptor))
        .collect();

    Arc::new(
        ScopeRegistry::new(HashMap::new(), components, Vec::new(), HashMap::new())
            .expect("candidate registry validates"),
    )
}

async fn empty_root() -> Arc<ScopeContainer> {
    ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), registry([]))
        .await
        .expect("empty root builds")
}

#[tokio::test]
async fn root_resolves_only_candidate_components() {
    let dependency =
        component::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);
    let consumer =
        component::<CandidateConsumer>("candidate-consumer", candidate_consumer_factories);

    let candidate = ScopeContainer::build_candidate_root(
        &[dependency.id, consumer.id],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([dependency, consumer]),
    )
    .await
    .expect("candidate root builds");
    let consumer = candidate
        .resolve::<Arc<CandidateConsumer>>()
        .await
        .expect("candidate resolves")
        .expect("candidate consumer exists");

    assert_eq!(consumer.dependency.0, "candidate");
}

#[tokio::test]
async fn root_never_falls_back_to_an_active_parent() {
    let dependency = ComponentDescriptor::manual(
        "active-dependency",
        "CandidateDependency",
        TypeDescriptor::of::<CandidateDependency>("CandidateDependency"),
        &Singleton,
    );
    let active = ScopeContainer::build_root(
        &[dependency],
        vec![BoxedComponent::new(CandidateDependency("active"))],
        ResolverSet::new(),
        registry([dependency]),
    )
    .await
    .expect("active root builds");
    let consumer =
        component::<CandidateConsumer>("candidate-consumer", candidate_consumer_factories);

    let result = ScopeContainer::build_candidate_root(
        &[consumer.id],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([consumer]),
    )
    .await;
    let Err(error) = result else {
        panic!("candidate cannot use active dependency");
    };

    assert!(matches!(
        error,
        Error::MissingComponent("CandidateDependency")
    ));
    assert_eq!(
        active
            .resolve::<Arc<CandidateDependency>>()
            .await
            .expect("active root resolves")
            .expect("active dependency exists")
            .0,
        "active"
    );
}

#[tokio::test]
async fn root_rejects_active_component_source() {
    let active = empty_root().await;
    let result = ScopeContainer::build_candidate_root(
        &[],
        Vec::new(),
        Vec::new(),
        active.resolvers().clone(),
        registry([]),
    )
    .await;
    let Err(error) = result else {
        panic!("active component source is rejected");
    };

    assert!(matches!(error, Error::CandidateActiveResolver));
}

#[tokio::test]
async fn factory_output_must_match_its_descriptor() {
    let descriptor =
        component::<CandidateDependency>("candidate-dependency", wrong_candidate_factories);
    let result = ScopeContainer::build_candidate_root(
        &[descriptor.id],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([descriptor]),
    )
    .await;
    let Err(error) = result else {
        panic!("wrong factory output is rejected");
    };

    assert!(matches!(error, Error::CandidateFactoryTypeMismatch { .. }));
}

#[tokio::test]
async fn factory_panics_are_redacted() {
    let descriptor =
        component::<CandidateDependency>("candidate-dependency", panicking_candidate_factories);
    let result = ScopeContainer::build_candidate_root(
        &[descriptor.id],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([descriptor]),
    )
    .await;
    let Err(error) = result else {
        panic!("factory panic is contained");
    };

    assert!(matches!(error, Error::CandidateFactoryPanicked { .. }));
    assert!(!error.to_string().contains("canary"));
}

#[tokio::test]
async fn builder_uses_the_candidate_registry_descriptor() {
    let active =
        component::<CandidateDependency>("candidate-dependency", panicking_candidate_factories);
    let candidate =
        component::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);

    let root = ScopeContainer::build_candidate_root(
        &[active.id],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([candidate]),
    )
    .await
    .expect("candidate registry factory is authoritative");
    let dependency = root
        .resolve::<Arc<CandidateDependency>>()
        .await
        .expect("candidate resolves")
        .expect("candidate dependency exists");

    assert_eq!(dependency.0, "candidate");
}

#[tokio::test]
async fn builder_rejects_an_incomplete_candidate_root() {
    let dependency =
        component::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);

    let result = ScopeContainer::build_candidate_root(
        &[],
        Vec::new(),
        Vec::new(),
        ResolverSet::new(),
        registry([dependency]),
    )
    .await;
    let Err(error) = result else {
        panic!("incomplete candidate root is rejected");
    };

    assert!(matches!(error, Error::CandidateComponentMismatch { .. }));
}

#[tokio::test]
async fn builder_rejects_a_retained_root_resolver() {
    let descriptor = crate::root_resolver_descriptor();
    let retained = BoxedComponent {
        ty: descriptor.ty,
        value: Box::new(Injectable::into_stored(crate::RootResolver::new())),
    };

    let result = ScopeContainer::build_candidate_root(
        &[],
        vec![retained],
        Vec::new(),
        ResolverSet::new(),
        registry([descriptor]),
    )
    .await;
    let Err(error) = result else {
        panic!("generation-bound root resolver cannot be retained");
    };

    assert!(matches!(
        error,
        Error::CandidateRuntimeBoundComponent { .. }
    ));
}

#[tokio::test]
async fn builder_recreates_and_attaches_the_root_resolver() {
    let target = ComponentDescriptor::manual(
        "candidate-dependency",
        "CandidateDependency",
        TypeDescriptor::of::<CandidateDependency>("CandidateDependency"),
        &Singleton,
    );
    let resolver = crate::root_resolver_descriptor();
    let retained = BoxedComponent::new(CandidateDependency("candidate"));

    let root = ScopeContainer::build_candidate_root(
        &[],
        vec![retained],
        Vec::new(),
        ResolverSet::new(),
        registry([target, resolver]),
    )
    .await
    .expect("root resolver is recreated");
    let resolver = root
        .get::<crate::RootResolver>()
        .expect("resolver is seeded");
    let target = resolver
        .component::<CandidateDependency>()
        .expect("resolver uses candidate root");

    assert_eq!(target.0, "candidate");
}

async fn carried_dependency(previous: crate::Previous<CandidateDependency>) -> CandidateDependency {
    CandidateDependency(previous.0)
}

fn replaceable_dependency_factories() -> &'static [ComponentFactoryDescriptor] {
    factories![
        "candidate" => candidate_dependency,
        replace "carry" => carried_dependency,
    ]()
}

#[tokio::test]
async fn previous_instances_are_built_by_their_replacement_factory() {
    let dependency =
        component::<CandidateDependency>("candidate-dependency", replaceable_dependency_factories);
    let consumer =
        component::<CandidateConsumer>("candidate-consumer", candidate_consumer_factories);

    let candidate = ScopeContainer::build_candidate_root(
        &[dependency.id, consumer.id],
        Vec::new(),
        vec![BoxedComponent::new(CandidateDependency("previous"))],
        ResolverSet::new(),
        registry([dependency, consumer]),
    )
    .await
    .expect("candidate root builds");
    let consumer = candidate
        .get::<CandidateConsumer>()
        .expect("candidate consumer exists");

    assert_eq!(
        consumer.dependency.0, "previous",
        "the replacement carried the previous instance; the consumer used its ordinary factory"
    );
}

#[tokio::test]
async fn previous_instances_need_a_replacement_factory_in_the_construction_order() {
    let replaceable =
        component::<CandidateDependency>("candidate-dependency", replaceable_dependency_factories);
    let ordinary =
        component::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);

    for (previous, descriptor) in [
        (BoxedComponent::new(WrongCandidate), replaceable),
        (
            BoxedComponent::new(CandidateDependency("previous")),
            ordinary,
        ),
    ] {
        let result = ScopeContainer::build_candidate_root(
            &[descriptor.id],
            Vec::new(),
            vec![previous],
            ResolverSet::new(),
            registry([descriptor]),
        )
        .await;

        assert!(
            matches!(result, Err(Error::CandidateComponentMismatch { .. })),
            "an unusable previous instance is rejected before construction"
        );
    }
}
