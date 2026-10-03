use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, DescriptorSource, Resolver, ResolverCtx, ResolverCtxExt,
    ResolverSet, RuntimeGenerationId, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    ComponentRegistry, ConditionFactSnapshot, EffectiveGraph, Injectable, ScopeContainer,
    ScopeRegistry, Singleton,
};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookKind, HookManager};

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
    generation_snapshot: None,
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
    prepared_with_hooks(condition, HookManager::new(Vec::new()), ResolverSet::new()).await
}

fn seed_hook_manager(hooks: &HookManager) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(hooks.clone())),
    }
}

async fn prepared_with_hooks(
    condition: Arc<AppConditionState>,
    hooks: HookManager,
    externals: ResolverSet,
) -> PreparedRuntimeGeneration {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );
    let root = ScopeContainer::build_root(
        &[],
        vec![seed_hook_manager(&hooks)],
        externals,
        Arc::clone(&registry),
    )
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
    RuntimeTransitionCoordinator::new(prepared_with_condition(condition).await)
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

#[tokio::test]
async fn stale_prepare_commit_rejects_without_running_the_compatibility_callback() {
    let coordinator = coordinator().await;
    let first = coordinator.begin().await;
    let committed = first
        .publish(prepared().await)
        .expect("first candidate publishes");
    let second = coordinator.begin().await;

    let callback_ran = Arc::new(AtomicBool::new(false));

    let error = second
        .prepare_commit(prepared().await)
        .map(|commit| {
            let callback_ran = Arc::clone(&callback_ran);
            commit.commit_with(move || callback_ran.store(true, Ordering::SeqCst))
        })
        .expect_err("candidate prepared from generation zero is stale");

    assert_eq!(
        error,
        StaleRuntimeProposal {
            attempt: TransitionAttemptId(2),
            base: RuntimeGenerationId::INITIAL,
            current: committed.id(),
        }
    );
    assert!(
        !callback_ran.load(Ordering::SeqCst),
        "a rejected candidate must never reach the compatibility callback"
    );
    assert_eq!(coordinator.current().id(), committed.id());

    let third = coordinator.begin().await;

    assert_eq!(third.attempt(), TransitionAttemptId(3));
    assert_eq!(third.base().id(), committed.id());
}

#[tokio::test]
async fn commit_with_publishes_before_the_compat_callback_and_holds_the_writer_through_it() {
    let coordinator = coordinator().await;
    let transition = coordinator.begin().await;
    let commit = transition
        .prepare_commit(prepared().await)
        .expect("current candidate prepares");

    let callback_entered = Arc::new(tokio::sync::Notify::new());
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let callback_calls = Arc::new(AtomicU64::new(0));
    let observed_current = Arc::new(AtomicU64::new(u64::MAX));

    let handle = {
        let callback_entered = Arc::clone(&callback_entered);
        let callback_calls = Arc::clone(&callback_calls);
        let observed_current = Arc::clone(&observed_current);
        let coordinator = coordinator.clone();

        tokio::task::spawn_blocking(move || {
            commit.commit_with(move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
                observed_current.store(coordinator.current().id().get(), Ordering::SeqCst);
                callback_entered.notify_one();

                let _ = release_rx.recv();
            })
        })
    };

    callback_entered.notified().await;

    assert_eq!(callback_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        observed_current.load(Ordering::SeqCst),
        RuntimeGenerationId::new(1).get(),
        "the candidate generation must already be current when the callback runs"
    );

    // Deterministic writer-lease check: the callback is blocked above, so the commit
    // token still owns the sole-writer guard and no contender can acquire the mutex.
    assert!(
        Arc::clone(&coordinator.writer).try_lock_owned().is_err(),
        "the writer mutex must stay locked while the compatibility callback runs"
    );

    release_tx.send(()).expect("commit task is still blocked");

    let committed = handle.await.expect("commit task does not panic");

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(
        committed.effective_graph().generation(),
        RuntimeGenerationId::new(1)
    );
    assert!(Arc::ptr_eq(
        &committed.generation,
        &coordinator.current().generation
    ));
    assert_eq!(callback_calls.load(Ordering::SeqCst), 1);

    let second = coordinator.begin().await;

    assert_eq!(second.attempt(), TransitionAttemptId(2));
    assert_eq!(second.base().id(), committed.id());
}

// ---------------------------------------------------------------------------
// Generation-local hook-manager routing.
// ---------------------------------------------------------------------------

/// A hook kind whose outcome reports which generation root resolved the hook.
struct ProbeKind;

impl HookKind for ProbeKind {
    type Output = &'static str;
    type Cx = ();

    const NAME: &'static str = "probe";
}

/// A resolver seeded into one generation's root, identifying it to probe hooks.
#[derive(Clone)]
struct GenerationProbe(&'static str);

impl Resolver for GenerationProbe {}

/// The hook's nominal receiver type; the probe call never resolves a component.
struct ProbeComponent;

fn probe_kind_ty() -> TypeId {
    TypeId::of::<ProbeKind>()
}

/// The boxed future shape of an erased hook call.
type ProbeCallFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

fn probe_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> ProbeCallFuture<'a> {
    Box::pin(async move {
        let tag = ctx
            .get_resolver::<GenerationProbe>()
            .map(|probe| probe.0)
            .unwrap_or("<unresolved>");

        Ok(Box::new(tag) as Box<dyn Any + Send>)
    })
}

static PROBE_HOOK: HookDescriptor = HookDescriptor::new(
    1,
    TypeDescriptor::of::<ProbeComponent>("ProbeComponent"),
    ProbeKind::NAME,
    probe_kind_ty,
    no_dependencies,
    probe_call,
);

fn probe_manager() -> HookManager {
    HookManager::new(vec![PROBE_HOOK])
}

fn probe_externals(tag: &'static str) -> ResolverSet {
    let mut externals = ResolverSet::new();

    externals.insert(Arc::new(GenerationProbe(tag)));

    externals
}

async fn probe_tags(hooks: &HookManager) -> Vec<&'static str> {
    hooks
        .run::<ProbeKind>(&(), |_| true)
        .await
        .into_iter()
        .map(|(_, outcome)| outcome.expect("probe hook resolves through its root"))
        .collect()
}

#[tokio::test]
async fn pinned_views_resolve_hooks_through_their_own_generation_manager() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("initial"),
        )
        .await,
    );
    let old = coordinator.current();

    let candidate = prepared_with_hooks(
        empty_condition(),
        probe_manager(),
        probe_externals("candidate"),
    )
    .await;
    let committed = coordinator
        .begin()
        .await
        .publish(candidate)
        .expect("candidate publishes");

    assert_eq!(probe_tags(old.hooks()).await, ["initial"]);
    assert_eq!(
        probe_tags(coordinator.current().hooks()).await,
        ["candidate"]
    );
    assert_eq!(probe_tags(committed.hooks()).await, ["candidate"]);
}

#[tokio::test]
async fn prepared_generation_binds_the_root_seeded_hook_manager() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("seeded"),
        )
        .await,
    );
    let view = coordinator.current();
    let root_hooks = view
        .root()
        .get::<HookManager>()
        .expect("every prepared root seeds the framework hook manager");

    assert!(root_hooks.has::<ProbeKind>());
    assert!(view.hooks().has::<ProbeKind>());
    assert!(root_hooks.component_has::<ProbeKind>(TypeId::of::<ProbeComponent>()));
    assert!(
        view.hooks()
            .component_has::<ProbeKind>(TypeId::of::<ProbeComponent>())
    );

    // The generation attached the root-resolved manager, so both handles route hook
    // receivers through this generation's root.
    assert_eq!(probe_tags(&root_hooks).await, ["seeded"]);
    assert_eq!(probe_tags(view.hooks()).await, ["seeded"]);
}

#[tokio::test]
async fn cloned_lifecycle_manager_survives_publication_while_its_view_is_held() {
    let coordinator = RuntimeTransitionCoordinator::new(
        prepared_with_hooks(
            empty_condition(),
            probe_manager(),
            probe_externals("initial"),
        )
        .await,
    );
    let lifecycle_view = coordinator.current();
    let hooks = lifecycle_view.hooks().clone();
    let root = Arc::downgrade(lifecycle_view.root());

    let candidate = prepared_with_hooks(
        empty_condition(),
        probe_manager(),
        probe_externals("candidate"),
    )
    .await;
    coordinator
        .begin()
        .await
        .publish(candidate)
        .expect("candidate publishes");

    // While the old view is held, the cloned manager still routes through its own root.
    assert_eq!(probe_tags(&hooks).await, ["initial"]);
    assert!(
        root.upgrade().is_some(),
        "the pinned view keeps the generation root alive"
    );

    // Dropping the view and the manager must release the root: the manager retains only
    // a weak reference, so no strong cycle keeps the generation alive.
    drop(hooks);
    drop(lifecycle_view);

    assert!(
        root.upgrade().is_none(),
        "the cloned manager must not retain its generation root"
    );
}
