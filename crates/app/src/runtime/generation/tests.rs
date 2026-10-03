use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, DescriptorSource, ResolverSet, RuntimeGenerationId,
    TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    ComponentRegistry, ConditionFactSnapshot, EffectiveGraph, ScopeContainer, ScopeRegistry,
    Singleton,
};
use upwell_hooks::HookManager;

use super::*;
use crate::{AppRegistry, ScopeTopology};

const ENABLED: ConfigFactId = ConfigFactId::new("test::GenerationConfig", "generation", "enabled");
const SOURCE: DescriptorSource = upwell_core::descriptor_source!();

static GENERATION_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "generation-enabled",
    source: SOURCE,
    predicate: ConditionPredicate::ConfigBool(ENABLED),
};

struct GenerationComponent;

fn no_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

fn unreachable_factory<'a>(
    _: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>> {
    Box::pin(async { unreachable!("generation fixtures never construct components") })
}

static GENERATION_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_factory,
    dependencies: no_dependencies,
    default: true,
}];

fn generation_factories() -> &'static [ComponentFactoryDescriptor] {
    &GENERATION_FACTORIES
}

static GENERATION_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: "generation_component",
    name: "GenerationComponent",
    ty: TypeDescriptor::of::<GenerationComponent>("GenerationComponent"),
    scope: &Singleton,
    condition: Some(&GENERATION_CONDITION),
    factories: generation_factories,
    hooks: upwell_hooks::no_hooks,
};

fn condition_facts() -> [ConfigFactDescriptor; 1] {
    [ConfigFactDescriptor {
        id: ENABLED,
        kind: ConditionScalarKind::Bool,
        source: SOURCE,
    }]
}

fn condition_snapshot(enabled: bool) -> ConditionFactSnapshot {
    ConditionFactSnapshot::new([(ENABLED, ConditionScalar::Bool(enabled))])
        .expect("facts are unique")
}

fn condition_registry() -> AppRegistry {
    AppRegistry {
        components: vec![GENERATION_COMPONENT],
        providers: Vec::new(),
        config_bindings: Vec::new(),
        condition_facts: Vec::new(),
    }
}

fn condition_state(enabled: bool) -> Arc<AppConditionState> {
    let registry = condition_registry();
    let evaluation = registry
        .evaluate_conditions(condition_facts(), &condition_snapshot(enabled))
        .expect("conditions evaluate");

    Arc::new(AppConditionState::new(Arc::new(registry), evaluation))
}

fn empty_condition() -> Arc<AppConditionState> {
    let registry = AppRegistry::default();
    let evaluation = registry
        .evaluate_conditions(
            [],
            &ConditionFactSnapshot::new([]).expect("empty snapshot validates"),
        )
        .expect("empty evaluation succeeds");

    Arc::new(AppConditionState::new(Arc::new(registry), evaluation))
}

async fn prepared() -> PreparedRuntimeGeneration {
    prepared_with_condition(empty_condition()).await
}

async fn prepared_with_condition(condition: Arc<AppConditionState>) -> PreparedRuntimeGeneration {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("empty root builds");
    let topology = Arc::new(
        ScopeTopology::empty()
            .prepare()
            .expect("empty topology prepares"),
    );
    let graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &ComponentRegistry::default(),
        |_, _| true,
    )
    .expect("empty graph validates");

    PreparedRuntimeGeneration::new(
        root,
        registry,
        RuntimeScopePlan::new(topology, Arc::new(HashMap::new()), Arc::new(HashMap::new())),
        Arc::from([]),
        graph,
        condition,
    )
}

async fn coordinator() -> RuntimeTransitionCoordinator {
    coordinator_with_condition(empty_condition()).await
}

async fn coordinator_with_condition(
    condition: Arc<AppConditionState>,
) -> RuntimeTransitionCoordinator {
    RuntimeTransitionCoordinator::new(
        prepared_with_condition(condition).await,
        HookManager::new(Vec::new()),
    )
}

#[tokio::test]
async fn no_op_attempts_advance_attempts_without_publishing_generations() {
    let coordinator = coordinator().await;
    let initial = coordinator.current();
    let first = coordinator.begin().await;

    assert_eq!(first.attempt().get(), 1);
    assert_eq!(first.base().id(), RuntimeGenerationId::INITIAL);

    let unchanged = first.finish_noop();
    let second = coordinator.begin().await;

    assert!(Arc::ptr_eq(&initial.generation, &unchanged.generation));
    assert!(Arc::ptr_eq(
        &unchanged.generation,
        &second.base().generation
    ));
    assert_eq!(second.attempt().get(), 2);
    assert_eq!(second.base().id(), RuntimeGenerationId::INITIAL);
}

#[tokio::test]
async fn publication_allocates_and_stamps_one_semantic_generation() {
    let coordinator = coordinator().await;
    let transition = coordinator.begin().await;

    let committed = transition
        .publish(prepared().await)
        .expect("current transition publishes");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        committed.effective_graph().generation(),
        RuntimeGenerationId::new(1)
    );
    assert!(Arc::ptr_eq(
        &committed.generation,
        &coordinator.current().generation
    ));
}

#[tokio::test]
async fn stale_base_cannot_replace_a_newer_generation() {
    let owner = Arc::new(());
    let publication = RuntimePublication::new(
        prepared()
            .await
            .commit(Arc::clone(&owner), RuntimeGenerationId::INITIAL),
    );
    let base = publication.pin();
    let winner = Arc::new(
        prepared()
            .await
            .commit(Arc::clone(&owner), RuntimeGenerationId::new(1)),
    );
    let stale = Arc::new(prepared().await.commit(owner, RuntimeGenerationId::new(2)));

    publication
        .publish(TransitionAttemptId(1), &base.generation, winner)
        .expect("winner publishes");
    let error = publication
        .publish(TransitionAttemptId(2), &base.generation, stale)
        .expect_err("stale candidate is rejected");

    assert_eq!(
        error,
        StaleRuntimeProposal {
            attempt: TransitionAttemptId(2),
            base: RuntimeGenerationId::INITIAL,
            current: RuntimeGenerationId::new(1),
        }
    );
}

#[tokio::test]
async fn publish_rejects_candidate_prepared_from_another_base() {
    let coordinator = coordinator().await;
    let first = coordinator.begin().await;
    let committed = first
        .publish(prepared().await)
        .expect("first candidate publishes");
    let second = coordinator.begin().await;

    let error = second
        .publish(prepared().await)
        .expect_err("candidate prepared from generation zero is stale");

    assert_eq!(
        error,
        StaleRuntimeProposal {
            attempt: TransitionAttemptId(2),
            base: RuntimeGenerationId::INITIAL,
            current: committed.id(),
        }
    );
    assert_eq!(coordinator.current().id(), committed.id());
}

#[tokio::test]
async fn coordinator_serializes_attempts_and_pins_the_latest_base() {
    let coordinator = coordinator().await;
    let first = coordinator.begin().await;
    let waiting = coordinator.clone();
    let second = tokio::spawn(async move { waiting.begin().await });

    tokio::task::yield_now().await;

    assert!(!second.is_finished());

    let committed = first
        .publish(prepared().await)
        .expect("first candidate publishes");
    let second = second.await.expect("second attempt starts");

    assert_eq!(second.attempt(), TransitionAttemptId(2));
    assert_eq!(second.base().id(), committed.id());
}

#[tokio::test]
async fn committed_condition_state_survives_initial_commit_and_publication() {
    let state = condition_state(true);
    let direct = condition_registry()
        .evaluate_conditions(condition_facts(), &condition_snapshot(true))
        .expect("direct evaluation succeeds");
    let eligible_ids = |evaluation: &crate::AppConditionEvaluation| {
        evaluation
            .evaluation()
            .eligible_registry()
            .components
            .iter()
            .map(|component| component.id)
            .collect::<Vec<_>>()
    };

    let coordinator = coordinator_with_condition(Arc::clone(&state)).await;
    let initial = coordinator.current();

    assert!(Arc::ptr_eq(&state, initial.condition()));
    assert_eq!(
        eligible_ids(initial.condition().evaluation()),
        eligible_ids(&direct)
    );

    let transition = coordinator.begin().await;
    let committed = transition
        .publish(prepared_with_condition(Arc::clone(&state)).await)
        .expect("current transition publishes");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        committed.effective_graph().generation(),
        RuntimeGenerationId::new(1)
    );
    assert!(Arc::ptr_eq(&state, committed.condition()));
}
