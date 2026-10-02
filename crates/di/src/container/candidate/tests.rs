use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, ResolverSet,
    TypeDescriptor,
};

use super::*;
use crate::{ComponentFactoryDescriptor, Injectable, ScopeContainer, ScopeRegistry, Singleton};

struct CandidateDependency(&'static str);

impl crate::Component for CandidateDependency {
    type Handle = Arc<Self>;

    const ID: &'static str = "candidate-dependency";
    const NAME: &'static str = "CandidateDependency";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

struct CandidateConsumer {
    dependency: Arc<CandidateDependency>,
}

struct WrongCandidate;

fn boxed_component<T: Send + Sync + 'static>(name: &'static str, value: Arc<T>) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<T>(name),
        value: Box::new(Injectable::into_stored(value)),
    }
}

fn dependency<T: ?Sized + 'static>() -> DependencyDescriptor {
    DependencyDescriptor {
        name: std::any::type_name::<T>(),
        ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn candidate_consumer_dependencies() -> Vec<DependencyDescriptor> {
    vec![dependency::<CandidateDependency>()]
}

fn candidate_dependency_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(boxed_component(
            "CandidateDependency",
            Arc::new(CandidateDependency("candidate")),
        ))
    })
}

fn candidate_consumer_factory(
    context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        let dependency = context
            .resolve::<Arc<CandidateDependency>>()
            .await?
            .ok_or(Error::MissingComponent("CandidateDependency"))?;

        Ok(boxed_component(
            "CandidateConsumer",
            Arc::new(CandidateConsumer { dependency }),
        ))
    })
}

fn wrong_candidate_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Ok(boxed_component("WrongCandidate", Arc::new(WrongCandidate))) })
}

fn panicking_candidate_factory(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { panic!("candidate canary") })
}

static CANDIDATE_DEPENDENCY_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "candidate",
        construct: candidate_dependency_factory,
        dependencies: no_dependencies,
        default: false,
    }];
static CANDIDATE_CONSUMER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "candidate",
    construct: candidate_consumer_factory,
    dependencies: candidate_consumer_dependencies,
    default: false,
}];
static WRONG_CANDIDATE_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "wrong",
    construct: wrong_candidate_factory,
    dependencies: no_dependencies,
    default: false,
}];
static PANICKING_CANDIDATE_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "panicking",
        construct: panicking_candidate_factory,
        dependencies: no_dependencies,
        default: false,
    }];

fn candidate_dependency_factories() -> &'static [ComponentFactoryDescriptor] {
    &CANDIDATE_DEPENDENCY_FACTORY
}

fn candidate_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &CANDIDATE_CONSUMER_FACTORY
}

fn wrong_candidate_factories() -> &'static [ComponentFactoryDescriptor] {
    &WRONG_CANDIDATE_FACTORY
}

fn panicking_candidate_factories() -> &'static [ComponentFactoryDescriptor] {
    &PANICKING_CANDIDATE_FACTORY
}

fn descriptor<T: 'static>(
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
    }
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
        descriptor::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);
    let consumer =
        descriptor::<CandidateConsumer>("candidate-consumer", candidate_consumer_factories);

    let candidate = ScopeContainer::build_candidate_root(
        &[dependency.id, consumer.id],
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
        vec![boxed_component(
            "CandidateDependency",
            Arc::new(CandidateDependency("active")),
        )],
        ResolverSet::new(),
        registry([dependency]),
    )
    .await
    .expect("active root builds");
    let consumer =
        descriptor::<CandidateConsumer>("candidate-consumer", candidate_consumer_factories);

    let result = ScopeContainer::build_candidate_root(
        &[consumer.id],
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
        descriptor::<CandidateDependency>("candidate-dependency", wrong_candidate_factories);
    let result = ScopeContainer::build_candidate_root(
        &[descriptor.id],
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
        descriptor::<CandidateDependency>("candidate-dependency", panicking_candidate_factories);
    let result = ScopeContainer::build_candidate_root(
        &[descriptor.id],
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
        descriptor::<CandidateDependency>("candidate-dependency", panicking_candidate_factories);
    let candidate =
        descriptor::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);

    let root = ScopeContainer::build_candidate_root(
        &[active.id],
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
        descriptor::<CandidateDependency>("candidate-dependency", candidate_dependency_factories);

    let result = ScopeContainer::build_candidate_root(
        &[],
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
    let retained = boxed_component(
        "CandidateDependency",
        Arc::new(CandidateDependency("candidate")),
    );

    let root = ScopeContainer::build_candidate_root(
        &[],
        vec![retained],
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
