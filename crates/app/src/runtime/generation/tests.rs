use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, RuntimeGenerationId};
use upwell_di::{ComponentRegistry, EffectiveGraph, ScopeContainer, ScopeRegistry};
use upwell_hooks::HookManager;

use super::*;
use crate::ScopeTopology;

async fn prepared() -> PreparedRuntimeGeneration {
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
    )
}

async fn coordinator() -> RuntimeTransitionCoordinator {
    RuntimeTransitionCoordinator::new(prepared().await, HookManager::new(Vec::new()))
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
