use std::any::{Any, TypeId};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_config::{ConfigBinding, ConfigManager, ConfigProperties, ConfigReloader};
use upwell_core::{
    Cardinality, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
    DependencyDescriptor, DependencyObservation, ResolutionMode, ResolverCtx, ResolverSet,
    RuntimeGenerationId, Transient, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, EffectiveGraph, EffectiveNodeRole, Fresh, FromContainer,
    Injectable, Live, NodeAction, PlannedNode, ProviderDescriptor, RootResolver, ScopeContainer,
    ScopeRegistry, Singleton, root_resolver_descriptor,
};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookDescriptor, HookKind, HookManager};

use super::{
    CandidateGraph, ComponentTransitionDecision, ComponentTransitionStrategy,
    ResolvedTransitionPlan, RestartReason,
};
use crate::runtime::{AppConditionState, AppRuntime, PreparedRuntimeGeneration, RuntimeScopePlan};
use crate::{AppRegistry, Error, RuntimeView, ScopeTopology};

const ENABLED: ConfigFactId = ConfigFactId::new("test::Settings", "settings", "enabled");
const SOURCE: upwell_core::DescriptorSource = upwell_core::descriptor_source!();

#[derive(serde::Deserialize)]
struct ForeignConfig;

impl ConfigProperties for ForeignConfig {
    const NAME: &'static str = "ForeignConfig";
}

fn facts() -> [ConfigFactDescriptor; 1] {
    [ConfigFactDescriptor {
        id: ENABLED,
        kind: ConditionScalarKind::Bool,
        source: SOURCE,
    }]
}

fn snapshot(enabled: bool) -> upwell_di::ConditionFactSnapshot {
    upwell_di::ConditionFactSnapshot::new([(ENABLED, ConditionScalar::Bool(enabled))])
        .expect("snapshot validates")
}

fn topology() -> crate::PreparedScopeTopology {
    ScopeTopology::empty()
        .prepare()
        .expect("empty topology validates")
}

#[test]
fn retained_live_rebinding_requires_restart_until_bindings_can_be_applied() {
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &AppRegistry::default(),
        &topology(),
    )
    .expect("empty candidate validates");
    let node = PlannedNode {
        component: "live-consumer",
        role: EffectiveNodeRole::Singleton,
        action: NodeAction::RebindLive,
        reasons: Box::new([]),
    };

    let error =
        super::validate_decision(&candidate, &node, Some(ComponentTransitionStrategy::Retain))
            .expect_err("live binding transitions are not applied yet");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "live-consumer",
            required: Some(NodeAction::RebindLive),
            reason: RestartReason::LiveRebindUnsupported,
        })
    ));
}

fn runtime_view(
    root: Arc<ScopeContainer>,
    scopes: Arc<ScopeRegistry>,
    resolved: Vec<ComponentDescriptor>,
    graph: EffectiveGraph,
) -> RuntimeView {
    let catalog = Arc::new(AppRegistry::default());
    let condition = Arc::new(AppConditionState::new(
        Arc::clone(&catalog),
        catalog
            .evaluate_conditions([], &upwell_di::ConditionFactSnapshot::default())
            .expect("empty condition state validates"),
    ));
    let generation = PreparedRuntimeGeneration::new(
        root,
        scopes,
        RuntimeScopePlan::new(
            Arc::new(topology()),
            Arc::new(HashMap::new()),
            Arc::new(HashMap::new()),
        ),
        Arc::from(resolved),
        graph,
        condition,
    );

    AppRuntime::new(
        Arc::from("transition-test"),
        generation,
        ConfigReloader::new(ConfigManager::empty(), Vec::new()),
    )
    .view()
}

#[test]
fn app_evaluation_prepares_a_candidate_without_components() {
    let registry = AppRegistry::default();
    let evaluation = registry
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");

    let candidate = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &registry,
        &evaluation,
        &topology(),
    )
    .expect("matching evaluation prepares");

    assert!(candidate.components().is_empty());
    assert!(candidate.singleton_order().is_empty());
}

#[test]
fn non_empty_registry_prepares_through_the_validated_graph_boundary() {
    let mut registry = AppRegistry::default();
    registry
        .components
        .push(upwell_di::root_resolver_descriptor());

    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("non-empty candidate prepares");

    assert_eq!(candidate.components().len(), 1);
    assert!(
        candidate
            .graph()
            .node(upwell_di::ROOT_RESOLVER_ID)
            .is_some()
    );
}

#[test]
fn evaluation_from_another_di_catalog_is_rejected() {
    let source = AppRegistry::default();
    let evaluation = source
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");
    let mut target = AppRegistry::default();
    target
        .components
        .push(upwell_di::root_resolver_descriptor());

    let result = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &target,
        &evaluation,
        &topology(),
    );
    let Err(error) = result else {
        panic!("foreign DI catalog is rejected");
    };

    assert!(matches!(
        error,
        Error::Condition(upwell_di::ConditionError::EvaluationCatalogMismatch)
    ));
}

#[test]
fn evaluation_from_another_app_binding_catalog_is_rejected() {
    let source = AppRegistry::default();
    let evaluation = source
        .evaluate_conditions(facts(), &snapshot(true))
        .expect("conditions evaluate");
    let mut target = AppRegistry::default();
    target
        .config_bindings
        .push(ConfigBinding::of::<ForeignConfig>("foreign"));

    let result = CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &target,
        &evaluation,
        &topology(),
    );
    let Err(error) = result else {
        panic!("foreign application binding catalog is rejected");
    };

    assert!(matches!(
        error,
        Error::ConditionEvaluationApplicationMismatch
    ));
}

#[test]
fn incremental_evaluation_rejects_raw_or_foreign_application_state() {
    let source = AppRegistry::default();
    let previous = source
        .evaluate_conditions(facts(), &snapshot(false))
        .expect("conditions evaluate");
    let mut registry = AppRegistry::default();
    registry
        .config_bindings
        .push(ConfigBinding::of::<ForeignConfig>("foreign"));

    let error = registry
        .evaluate_changed_conditions(facts(), &previous, &snapshot(true))
        .expect_err("incremental evaluation cannot launder application identity");

    assert!(matches!(
        error,
        Error::ConditionEvaluationApplicationMismatch
    ));
}

#[test]
fn incremental_app_evaluation_remains_accepted_by_candidate_preparation() {
    let registry = AppRegistry::default();
    let initial = registry
        .evaluate_conditions(facts(), &snapshot(false))
        .expect("initial conditions evaluate");
    let changed = registry
        .evaluate_changed_conditions(facts(), &initial, &snapshot(true))
        .expect("incremental conditions evaluate");

    CandidateGraph::prepare_evaluation(
        RuntimeGenerationId::INITIAL,
        &registry,
        &changed,
        &topology(),
    )
    .expect("incremental evaluation retains application identity");
}

struct Replaceable {
    label: &'static str,
}

struct RootBoundConsumer;

fn construct_active_replaceable(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<Replaceable>("Replaceable"),
            value: Box::new(Injectable::into_stored(Arc::new(Replaceable {
                label: "active",
            }))),
        })
    })
}

fn construct_candidate_replaceable(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<Replaceable>("Replaceable"),
            value: Box::new(Injectable::into_stored(Arc::new(Replaceable {
                label: "candidate",
            }))),
        })
    })
}

fn no_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

fn root_resolver_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "RootResolver",
        ty: TypeDescriptor::of::<upwell_di::RootResolver>("RootResolver"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

static ACTIVE_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "active",
    construct: construct_active_replaceable,
    dependencies: no_dependencies,
    default: false,
}];
static CANDIDATE_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "candidate",
    construct: construct_candidate_replaceable,
    dependencies: no_dependencies,
    default: false,
}];
static ROOT_BOUND_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "root-bound",
    construct: construct_active_replaceable,
    dependencies: root_resolver_dependencies,
    default: false,
}];

fn active_factories() -> &'static [ComponentFactoryDescriptor] {
    &ACTIVE_FACTORY
}

fn candidate_factories() -> &'static [ComponentFactoryDescriptor] {
    &CANDIDATE_FACTORY
}

fn root_bound_factories() -> &'static [ComponentFactoryDescriptor] {
    &ROOT_BOUND_FACTORY
}

fn replaceable(factories: fn() -> &'static [ComponentFactoryDescriptor]) -> ComponentDescriptor {
    ComponentDescriptor {
        id: "replaceable",
        name: "Replaceable",
        ty: TypeDescriptor::of::<Replaceable>("Replaceable"),
        scope: &Singleton,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn strategy_fixture() -> (EffectiveGraph, CandidateGraph) {
    let mut active = AppRegistry::default();
    active.components.push(replaceable(active_factories));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let mut candidate = AppRegistry::default();
    candidate.components.push(replaceable(candidate_factories));
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");

    (active, candidate)
}

#[test]
fn replacement_requires_an_explicit_component_decision() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(&active, [])
        .expect_err("missing component decision requires restart");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            reason: RestartReason::MissingDecision,
            ..
        })
    ));
}

#[test]
fn replacement_cannot_retain_a_stale_instance() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Retain,
            }],
        )
        .expect_err("structural replacement cannot retain");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            reason: RestartReason::StaleRetention,
            ..
        })
    ));
}

#[test]
fn explicit_reconstruction_uses_candidate_graph_order() {
    let (active, candidate) = strategy_fixture();

    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    assert_eq!(resolved.construction_order(), ["replaceable"]);
    assert_eq!(resolved.decisions().len(), 1);
}

#[test]
fn duplicate_component_decisions_are_rejected() {
    let (active, candidate) = strategy_fixture();
    let decision = ComponentTransitionDecision {
        component: "replaceable",
        strategy: ComponentTransitionStrategy::Reconstruct,
    };

    let error = candidate
        .resolve_transition(&active, [decision, decision])
        .expect_err("duplicate component decisions are invalid");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            reason: RestartReason::DuplicateDecision,
            ..
        })
    ));
}

#[test]
fn unknown_component_decisions_are_rejected() {
    let (active, candidate) = strategy_fixture();

    let error = candidate
        .resolve_transition(
            &active,
            [
                ComponentTransitionDecision {
                    component: "replaceable",
                    strategy: ComponentTransitionStrategy::Reconstruct,
                },
                ComponentTransitionDecision {
                    component: "unknown",
                    strategy: ComponentTransitionStrategy::Retain,
                },
            ],
        )
        .expect_err("unknown component decision is invalid");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            reason: RestartReason::UnknownDecision,
            ..
        })
    ));
}

#[tokio::test]
async fn resolved_plan_builds_only_the_candidate_factory() {
    let (active, candidate) = strategy_fixture();
    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    let (root, scopes) = resolved
        .build_candidate_root(&candidate, Vec::new(), ResolverSet::new())
        .await
        .expect("resolved plan constructs candidate root");

    assert!(root.belongs_to_registry(&scopes));
    assert!(root.resolve::<std::sync::Arc<Replaceable>>().await.is_ok());
}

#[tokio::test]
async fn resolved_plan_cannot_build_another_candidate() {
    let (active, candidate) = strategy_fixture();
    let (_, other_candidate) = strategy_fixture();
    let resolved = candidate
        .resolve_transition(
            &active,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("ordinary reconstruction is supported");

    let result = resolved
        .build_candidate_root(&other_candidate, Vec::new(), ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("resolved plan is bound to its candidate");
    };

    assert!(matches!(error, Error::StaleGraphCandidate(_)));
}

#[tokio::test]
async fn retained_values_must_match_complete_plan_dispositions() {
    let mut active = AppRegistry::default();
    active.components.push(replaceable(active_factories));
    active.components.push(ComponentDescriptor::manual(
        "retained",
        "Retained",
        TypeDescriptor::of::<u8>("Retained"),
        &Singleton,
    ));
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let mut candidate = AppRegistry::default();
    candidate.components.push(replaceable(candidate_factories));
    candidate.components.push(ComponentDescriptor::manual(
        "retained",
        "Retained",
        TypeDescriptor::of::<u8>("Retained"),
        &Singleton,
    ));
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("unchanged component receives retain disposition");

    let result = resolved
        .build_candidate_root(&candidate, Vec::new(), ResolverSet::new())
        .await;
    let Err(error) = result else {
        panic!("missing retained value is rejected");
    };

    assert!(
        matches!(
            error,
            Error::RestartRequired(super::RestartRequired {
                reason: RestartReason::MissingDecision,
                ..
            })
        ),
        "the low-level seam must still enforce complete plan dispositions"
    );
}

#[test]
fn unchanged_root_resolver_consumers_require_reconstruction() {
    let root_bound = ComponentDescriptor {
        id: "root-bound",
        name: "RootBoundConsumer",
        ty: TypeDescriptor::of::<RootBoundConsumer>("RootBoundConsumer"),
        scope: &Singleton,
        condition: None,
        factories: root_bound_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let mut active = AppRegistry::default();
    active.components.extend([
        replaceable(active_factories),
        upwell_di::root_resolver_descriptor(),
        root_bound,
    ]);
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let mut candidate = AppRegistry::default();
    candidate.components.extend([
        replaceable(candidate_factories),
        upwell_di::root_resolver_descriptor(),
        root_bound,
    ]);
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");

    let error = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect_err("root-bound consumer cannot be retained");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            reason: RestartReason::GenerationBoundDependency,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Runtime invalidation defaults for the transactional config reload.
// ---------------------------------------------------------------------------

struct InvalidatedRoot;

struct FixedDependent;

struct UnrelatedSingleton;

struct ManualRoot;

fn construct_invalidated_root(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<InvalidatedRoot>("InvalidatedRoot"),
            value: Box::new(Injectable::into_stored(Arc::new(InvalidatedRoot))),
        })
    })
}

fn construct_fixed_dependent(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<FixedDependent>("FixedDependent"),
            value: Box::new(Injectable::into_stored(Arc::new(FixedDependent))),
        })
    })
}

fn construct_unrelated_singleton(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<UnrelatedSingleton>("UnrelatedSingleton"),
            value: Box::new(Injectable::into_stored(Arc::new(UnrelatedSingleton))),
        })
    })
}

fn invalidated_root_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "InvalidatedRoot",
        ty: TypeDescriptor::of::<InvalidatedRoot>("InvalidatedRoot"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

static INVALIDATED_ROOT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "invalidated-root",
    construct: construct_invalidated_root,
    dependencies: no_dependencies,
    default: false,
}];
static FIXED_DEPENDENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "fixed-dependent",
    construct: construct_fixed_dependent,
    dependencies: invalidated_root_dependency,
    default: false,
}];
static UNRELATED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "unrelated",
    construct: construct_unrelated_singleton,
    dependencies: no_dependencies,
    default: false,
}];

fn invalidated_root_factories() -> &'static [ComponentFactoryDescriptor] {
    &INVALIDATED_ROOT_FACTORY
}

fn fixed_dependent_factories() -> &'static [ComponentFactoryDescriptor] {
    &FIXED_DEPENDENT_FACTORY
}

fn unrelated_factories() -> &'static [ComponentFactoryDescriptor] {
    &UNRELATED_FACTORY
}

fn invalidated_root() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "invalidated-root",
        name: "InvalidatedRoot",
        ty: TypeDescriptor::of::<InvalidatedRoot>("InvalidatedRoot"),
        scope: &Singleton,
        condition: None,
        factories: invalidated_root_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn fixed_dependent() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "fixed-dependent",
        name: "FixedDependent",
        ty: TypeDescriptor::of::<FixedDependent>("FixedDependent"),
        scope: &Singleton,
        condition: None,
        factories: fixed_dependent_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn unrelated_singleton() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "unrelated",
        name: "UnrelatedSingleton",
        ty: TypeDescriptor::of::<UnrelatedSingleton>("UnrelatedSingleton"),
        scope: &Singleton,
        condition: None,
        factories: unrelated_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

/// Identical active and candidate graphs, so only runtime invalidations can force
/// transition work.
fn invalidation_fixture() -> (EffectiveGraph, CandidateGraph) {
    let mut registry = AppRegistry::default();
    registry
        .components
        .extend([invalidated_root(), fixed_dependent(), unrelated_singleton()]);
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates");

    (active, candidate)
}

#[test]
fn runtime_invalidations_reconstruct_invalidated_root_and_fixed_dependent() {
    let (active, candidate) = invalidation_fixture();
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("runtime invalidations resolve without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 3);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "invalidated-root",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "fixed-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Retain,
    }));
    assert_eq!(
        resolved.construction_order(),
        ["invalidated-root", "fixed-dependent"]
    );
}

#[test]
fn runtime_invalidated_factoryless_singleton_requires_restart() {
    let mut registry = AppRegistry::default();
    registry.components.push(ComponentDescriptor::manual(
        "manual-root",
        "ManualRoot",
        TypeDescriptor::of::<ManualRoot>("ManualRoot"),
        &Singleton,
    ));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates");
    let invalidated = BTreeSet::from(["manual-root"]);

    let error = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect_err("a factoryless invalidated singleton cannot reconstruct");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "manual-root",
            required: Some(NodeAction::Replace),
            reason: RestartReason::FactoryUnavailable,
        })
    ));
}

struct CountedTransient;

static TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

fn construct_counted_transient(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<CountedTransient>("CountedTransient"),
            value: Box::new(Injectable::into_stored(Arc::new(CountedTransient))),
        })
    })
}

static COUNTED_TRANSIENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "counted-transient",
    construct: construct_counted_transient,
    dependencies: no_dependencies,
    default: false,
}];

fn counted_transient_factories() -> &'static [ComponentFactoryDescriptor] {
    &COUNTED_TRANSIENT_FACTORY
}

fn counted_transient() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "counted-transient",
        name: "CountedTransient",
        ty: TypeDescriptor::of::<CountedTransient>("CountedTransient"),
        scope: &Transient,
        condition: None,
        factories: counted_transient_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

#[test]
fn runtime_invalidated_non_singleton_requires_restart_before_construction() {
    let mut registry = AppRegistry::default();
    registry.components.push(counted_transient());
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates");
    let invalidated = BTreeSet::from(["counted-transient"]);

    let error = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect_err("an invalidated non-singleton cannot join a runtime transition");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "counted-transient",
            required: Some(NodeAction::Replace),
            reason: RestartReason::NonSingleton,
        })
    ));
    assert_eq!(
        TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution must reject before any factory callback runs"
    );
}

struct LiveInvalidatedConsumer;

struct LiveConsumerDependent;

struct ClosureTransient;

fn construct_live_invalidated_consumer(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<LiveInvalidatedConsumer>("LiveInvalidatedConsumer"),
            value: Box::new(Injectable::into_stored(Arc::new(LiveInvalidatedConsumer))),
        })
    })
}

fn construct_live_consumer_dependent(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<LiveConsumerDependent>("LiveConsumerDependent"),
            value: Box::new(Injectable::into_stored(Arc::new(LiveConsumerDependent))),
        })
    })
}

static CLOSURE_TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

fn construct_closure_transient(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        CLOSURE_TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ClosureTransient>("ClosureTransient"),
            value: Box::new(Injectable::into_stored(Arc::new(ClosureTransient))),
        })
    })
}

fn invalidated_root_live_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "InvalidatedRoot",
        ty: TypeDescriptor::of::<InvalidatedRoot>("InvalidatedRoot"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Live,
    }]
}

fn live_consumer_snapshot_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "LiveInvalidatedConsumer",
        ty: TypeDescriptor::of::<LiveInvalidatedConsumer>("LiveInvalidatedConsumer"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

static LIVE_INVALIDATED_CONSUMER_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "live-invalidated-consumer",
        construct: construct_live_invalidated_consumer,
        dependencies: invalidated_root_live_dependency,
        default: false,
    }];
static LIVE_CONSUMER_DEPENDENT_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "live-consumer-dependent",
        construct: construct_live_consumer_dependent,
        dependencies: live_consumer_snapshot_dependency,
        default: false,
    }];
static CLOSURE_TRANSIENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "closure-transient",
    construct: construct_closure_transient,
    dependencies: live_consumer_snapshot_dependency,
    default: false,
}];

fn live_invalidated_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_INVALIDATED_CONSUMER_FACTORY
}

fn live_consumer_dependent_factories() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_CONSUMER_DEPENDENT_FACTORY
}

fn closure_transient_factories() -> &'static [ComponentFactoryDescriptor] {
    &CLOSURE_TRANSIENT_FACTORY
}

fn live_invalidated_consumer() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "live-invalidated-consumer",
        name: "LiveInvalidatedConsumer",
        ty: TypeDescriptor::of::<LiveInvalidatedConsumer>("LiveInvalidatedConsumer"),
        scope: &Singleton,
        condition: None,
        factories: live_invalidated_consumer_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn live_consumer_dependent() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "live-consumer-dependent",
        name: "LiveConsumerDependent",
        ty: TypeDescriptor::of::<LiveConsumerDependent>("LiveConsumerDependent"),
        scope: &Singleton,
        condition: None,
        factories: live_consumer_dependent_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn closure_transient() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "closure-transient",
        name: "ClosureTransient",
        ty: TypeDescriptor::of::<ClosureTransient>("ClosureTransient"),
        scope: &Transient,
        condition: None,
        factories: closure_transient_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

/// Identical active and candidate graphs where the invalidated root feeds a live
/// consumer whose own snapshot dependents must join the reconstruction closure.
fn live_invalidation_fixture() -> (EffectiveGraph, CandidateGraph) {
    let mut registry = AppRegistry::default();
    registry.components.extend([
        invalidated_root(),
        live_invalidated_consumer(),
        live_consumer_dependent(),
        unrelated_singleton(),
    ]);
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates");

    (active, candidate)
}

#[test]
fn runtime_resolver_maps_the_invalidated_live_closure_to_reconstruct() {
    let (active, candidate) = live_invalidation_fixture();
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let resolved = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect("the invalidated live closure resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 4);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "invalidated-root",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-invalidated-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Retain,
    }));
    assert_eq!(
        resolved.construction_order(),
        [
            "invalidated-root",
            "live-invalidated-consumer",
            "live-consumer-dependent"
        ]
    );
}

#[test]
fn runtime_invalidated_non_reconstructible_dependent_requires_restart_before_construction() {
    let mut registry = AppRegistry::default();
    registry.components.extend([
        invalidated_root(),
        live_invalidated_consumer(),
        closure_transient(),
    ]);
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates");
    let invalidated = BTreeSet::from(["invalidated-root"]);

    let error = candidate
        .resolve_runtime_transition(&active, &invalidated)
        .expect_err("a non-singleton closure dependent cannot join a runtime transition");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "closure-transient",
            required: Some(NodeAction::Replace),
            reason: RestartReason::NonSingleton,
        })
    ));
    assert_eq!(
        CLOSURE_TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution must reject before any factory callback runs"
    );
}

// ---------------------------------------------------------------------------
// Runtime resolver mapping of structural Add, RebindLive, and Remove actions.
// ---------------------------------------------------------------------------

trait LiveService: Send + Sync {}

struct LiveProviderA;

struct LiveProviderB;

struct LiveConsumer;

fn construct_live_provider_a(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<LiveProviderA>("LiveProviderA"),
            value: Box::new(Injectable::into_stored(Arc::new(LiveProviderA))),
        })
    })
}

fn construct_live_provider_b(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<LiveProviderB>("LiveProviderB"),
            value: Box::new(Injectable::into_stored(Arc::new(LiveProviderB))),
        })
    })
}

fn construct_live_consumer(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<LiveConsumer>("LiveConsumer"),
            value: Box::new(Injectable::into_stored(Arc::new(LiveConsumer))),
        })
    })
}

fn live_service_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "LiveService",
        ty: TypeDescriptor::of::<dyn LiveService>("dyn LiveService"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Live,
    }]
}

static LIVE_PROVIDER_A_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "live-provider-a",
    construct: construct_live_provider_a,
    dependencies: no_dependencies,
    default: false,
}];
static LIVE_PROVIDER_B_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "live-provider-b",
    construct: construct_live_provider_b,
    dependencies: no_dependencies,
    default: false,
}];
static LIVE_CONSUMER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "live-consumer",
    construct: construct_live_consumer,
    dependencies: live_service_dependency,
    default: false,
}];

fn live_provider_a_factories() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_PROVIDER_A_FACTORY
}

fn live_provider_b_factories() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_PROVIDER_B_FACTORY
}

fn live_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &LIVE_CONSUMER_FACTORY
}

fn live_provider_a() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "live-provider-a",
        name: "LiveProviderA",
        ty: TypeDescriptor::of::<LiveProviderA>("LiveProviderA"),
        scope: &Singleton,
        condition: None,
        factories: live_provider_a_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn live_provider_b() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "live-provider-b",
        name: "LiveProviderB",
        ty: TypeDescriptor::of::<LiveProviderB>("LiveProviderB"),
        scope: &Singleton,
        condition: None,
        factories: live_provider_b_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn live_consumer() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "live-consumer",
        name: "LiveConsumer",
        ty: TypeDescriptor::of::<LiveConsumer>("LiveConsumer"),
        scope: &Singleton,
        condition: None,
        factories: live_consumer_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn live_service_provider(
    concrete_ty: TypeDescriptor,
    erase: fn(&BoxedComponent) -> BoxedComponent,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn LiveService>("dyn LiveService"),
        concrete_ty,
        qualifier: "live",
        primary: false,
        priority: 0,
        ordering: &[],
        erase,
    }
}

/// Active serves the live dependency through provider A; the candidate retargets the
/// identical live dependency to provider B, so the structural plan removes A, adds B,
/// and rebinds the consumer live.
fn live_retarget_fixture() -> (EffectiveGraph, CandidateGraph) {
    let mut active_registry = AppRegistry::default();
    active_registry
        .components
        .extend([live_provider_a(), live_consumer()]);
    active_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderA>("LiveProviderA"),
        |_| panic!("transition planning must not erase components"),
    ));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active_registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");

    let mut candidate_registry = AppRegistry::default();
    candidate_registry
        .components
        .extend([live_provider_b(), live_consumer()]);
    candidate_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderB>("LiveProviderB"),
        |_| panic!("transition planning must not erase components"),
    ));
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        &topology(),
    )
    .expect("candidate graph validates");

    (active, candidate)
}

#[test]
fn runtime_resolver_maps_structural_add_to_reconstruct() {
    let mut active_registry = AppRegistry::default();
    active_registry
        .components
        .push(replaceable(active_factories));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active_registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");

    let mut candidate_registry = AppRegistry::default();
    candidate_registry
        .components
        .extend([replaceable(candidate_factories), unrelated_singleton()]);
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        &topology(),
    )
    .expect("candidate graph validates");

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a structural add resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 2);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "unrelated",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "replaceable",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
}

#[test]
fn runtime_resolver_maps_rebind_live_to_reconstruct() {
    let (active, candidate) = live_retarget_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a live retarget resolves without restart");
    let decisions = resolved.decisions();

    assert_eq!(decisions.len(), 2);
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-provider-b",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
}

#[test]
fn runtime_resolver_omits_removed_components_by_absence() {
    let (active, candidate) = live_retarget_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("a removal retires by absence without a restart");

    assert!(matches!(
        resolved
            .structural()
            .nodes
            .iter()
            .find(|node| node.component == "live-provider-a"),
        Some(PlannedNode {
            action: NodeAction::Remove,
            ..
        })
    ));
    assert!(
        !resolved
            .decisions()
            .iter()
            .any(|decision| decision.component == "live-provider-a"),
        "a removed component carries no decision and requires no candidate seed"
    );
}

// ---------------------------------------------------------------------------
// Runtime propagation of live retarget closures under an empty invalidation set.
// ---------------------------------------------------------------------------

struct RetargetDependent;

struct RetargetChainConsumer;

struct RetargetChainDependent;

struct RetargetClosureTransient;

static RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

fn construct_retarget_dependent(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<RetargetDependent>("RetargetDependent"),
            value: Box::new(Injectable::into_stored(Arc::new(RetargetDependent))),
        })
    })
}

fn construct_retarget_chain_consumer(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<RetargetChainConsumer>("RetargetChainConsumer"),
            value: Box::new(Injectable::into_stored(Arc::new(RetargetChainConsumer))),
        })
    })
}

fn construct_retarget_chain_dependent(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<RetargetChainDependent>("RetargetChainDependent"),
            value: Box::new(Injectable::into_stored(Arc::new(RetargetChainDependent))),
        })
    })
}

fn construct_retarget_closure_transient(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<RetargetClosureTransient>("RetargetClosureTransient"),
            value: Box::new(Injectable::into_stored(Arc::new(RetargetClosureTransient))),
        })
    })
}

fn retarget_dependent_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "LiveConsumer",
        ty: TypeDescriptor::of::<LiveConsumer>("LiveConsumer"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

fn retarget_chain_consumer_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "RetargetDependent",
        ty: TypeDescriptor::of::<RetargetDependent>("RetargetDependent"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Live,
    }]
}

fn retarget_chain_dependent_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "RetargetChainConsumer",
        ty: TypeDescriptor::of::<RetargetChainConsumer>("RetargetChainConsumer"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

fn retarget_closure_transient_dependencies() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: "LiveConsumer",
        ty: TypeDescriptor::of::<LiveConsumer>("LiveConsumer"),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

static RETARGET_DEPENDENT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "retarget-dependent",
    construct: construct_retarget_dependent,
    dependencies: retarget_dependent_dependencies,
    default: false,
}];
static RETARGET_CHAIN_CONSUMER_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "retarget-chain-consumer",
        construct: construct_retarget_chain_consumer,
        dependencies: retarget_chain_consumer_dependencies,
        default: false,
    }];
static RETARGET_CHAIN_DEPENDENT_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "retarget-chain-dependent",
        construct: construct_retarget_chain_dependent,
        dependencies: retarget_chain_dependent_dependencies,
        default: false,
    }];
static RETARGET_CLOSURE_TRANSIENT_FACTORY: [ComponentFactoryDescriptor; 1] =
    [ComponentFactoryDescriptor {
        id: "retarget-closure-transient",
        construct: construct_retarget_closure_transient,
        dependencies: retarget_closure_transient_dependencies,
        default: false,
    }];

fn retarget_dependent_factories() -> &'static [ComponentFactoryDescriptor] {
    &RETARGET_DEPENDENT_FACTORY
}

fn retarget_chain_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &RETARGET_CHAIN_CONSUMER_FACTORY
}

fn retarget_chain_dependent_factories() -> &'static [ComponentFactoryDescriptor] {
    &RETARGET_CHAIN_DEPENDENT_FACTORY
}

fn retarget_closure_transient_factories() -> &'static [ComponentFactoryDescriptor] {
    &RETARGET_CLOSURE_TRANSIENT_FACTORY
}

fn retarget_dependent() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "retarget-dependent",
        name: "RetargetDependent",
        ty: TypeDescriptor::of::<RetargetDependent>("RetargetDependent"),
        scope: &Singleton,
        condition: None,
        factories: retarget_dependent_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn retarget_chain_consumer() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "retarget-chain-consumer",
        name: "RetargetChainConsumer",
        ty: TypeDescriptor::of::<RetargetChainConsumer>("RetargetChainConsumer"),
        scope: &Singleton,
        condition: None,
        factories: retarget_chain_consumer_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn retarget_chain_dependent() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "retarget-chain-dependent",
        name: "RetargetChainDependent",
        ty: TypeDescriptor::of::<RetargetChainDependent>("RetargetChainDependent"),
        scope: &Singleton,
        condition: None,
        factories: retarget_chain_dependent_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn retarget_closure_transient() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "retarget-closure-transient",
        name: "RetargetClosureTransient",
        ty: TypeDescriptor::of::<RetargetClosureTransient>("RetargetClosureTransient"),
        scope: &Transient,
        condition: None,
        factories: retarget_closure_transient_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

/// Active serves the live dependency through provider A; the candidate retargets the
/// identical live dependency to provider B. The live consumer's snapshot dependent and
/// the live chain behind it are structurally untouched, so only runtime propagation
/// can pull them into the reconstruction closure.
fn live_retarget_closure_fixture() -> (EffectiveGraph, CandidateGraph) {
    let mut active_registry = AppRegistry::default();
    active_registry.components.extend([
        live_provider_a(),
        live_consumer(),
        retarget_dependent(),
        retarget_chain_consumer(),
        retarget_chain_dependent(),
    ]);
    active_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderA>("LiveProviderA"),
        |_| panic!("transition planning must not erase components"),
    ));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active_registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");

    let mut candidate_registry = AppRegistry::default();
    candidate_registry.components.extend([
        live_provider_b(),
        live_consumer(),
        retarget_dependent(),
        retarget_chain_consumer(),
        retarget_chain_dependent(),
    ]);
    candidate_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderB>("LiveProviderB"),
        |_| panic!("transition planning must not erase components"),
    ));
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        &topology(),
    )
    .expect("candidate graph validates");

    (active, candidate)
}

#[test]
fn runtime_resolver_propagates_the_live_retarget_closure_with_empty_invalidations() {
    let (active, candidate) = live_retarget_closure_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("the live retarget closure resolves without restart");
    let decisions = resolved.decisions();

    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "live-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(
        decisions.contains(&ComponentTransitionDecision {
            component: "retarget-dependent",
            strategy: ComponentTransitionStrategy::Reconstruct,
        }),
        "the snapshot dependent of the reconstructed live consumer must reconstruct, \
         not retain an instance snapshotted from the retired active root"
    );

    let structural = resolved.structural();

    assert!(matches!(
        structural
            .nodes
            .iter()
            .find(|node| node.component == "live-consumer"),
        Some(PlannedNode {
            action: NodeAction::RebindLive,
            ..
        })
    ));
    assert!(matches!(
        structural
            .nodes
            .iter()
            .find(|node| node.component == "retarget-dependent"),
        Some(PlannedNode {
            action: NodeAction::Replace,
            ..
        })
    ));
}

#[test]
fn runtime_resolver_propagates_repeated_live_chains_with_empty_invalidations() {
    let (active, candidate) = live_retarget_closure_fixture();

    let resolved = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect("the repeated live chain resolves without restart");
    let decisions = resolved.decisions();

    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "retarget-chain-consumer",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert!(decisions.contains(&ComponentTransitionDecision {
        component: "retarget-chain-dependent",
        strategy: ComponentTransitionStrategy::Reconstruct,
    }));
    assert_eq!(
        resolved.construction_order(),
        [
            "live-provider-b",
            "live-consumer",
            "retarget-dependent",
            "retarget-chain-consumer",
            "retarget-chain-dependent",
        ],
        "the propagated closure reconstructs in dependency order"
    );
}

#[test]
fn runtime_resolver_preflights_non_reconstructible_live_retarget_closure() {
    let mut active_registry = AppRegistry::default();
    active_registry.components.extend([
        live_provider_a(),
        live_consumer(),
        retarget_closure_transient(),
    ]);
    active_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderA>("LiveProviderA"),
        |_| panic!("transition planning must not erase components"),
    ));
    let active = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active_registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");

    let mut candidate_registry = AppRegistry::default();
    candidate_registry.components.extend([
        live_provider_b(),
        live_consumer(),
        retarget_closure_transient(),
    ]);
    candidate_registry.providers.push(live_service_provider(
        TypeDescriptor::of::<LiveProviderB>("LiveProviderB"),
        |_| panic!("transition planning must not erase components"),
    ));
    let candidate = CandidateGraph::prepare(
        RuntimeGenerationId::INITIAL,
        &candidate_registry,
        &topology(),
    )
    .expect("candidate graph validates");

    let error = candidate
        .resolve_runtime_transition(&active, &BTreeSet::new())
        .expect_err("a non-singleton closure dependent cannot join a runtime transition");

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "retarget-closure-transient",
            required: Some(NodeAction::Replace),
            reason: RestartReason::NonSingleton,
        })
    ));
    assert_eq!(
        RETARGET_CLOSURE_TRANSIENT_CONSTRUCTIONS.load(Ordering::SeqCst),
        0,
        "resolution must reject before any factory callback runs"
    );
}

#[derive(Clone)]
struct TransientSeed;

impl Injectable for TransientSeed {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[derive(Clone)]
struct FreshHolder {
    fresh: Fresh<TransientSeed>,
}

impl Injectable for FreshHolder {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

fn construct_transient_seed(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<TransientSeed>("TransientSeed"),
            value: Box::new(Injectable::into_stored(TransientSeed)),
        })
    })
}

fn construct_fresh_holder(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        let fresh = <Fresh<TransientSeed> as FromContainer>::from_container(cx).await?;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<FreshHolder>("FreshHolder"),
            value: Box::new(Injectable::into_stored(std::sync::Arc::new(FreshHolder {
                fresh,
            }))),
        })
    })
}

static TRANSIENT_SEED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "seed",
    construct: construct_transient_seed,
    dependencies: no_dependencies,
    default: false,
}];
static ACTIVE_HOLDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "holder-active",
    construct: construct_fresh_holder,
    dependencies: no_dependencies,
    default: false,
}];
static CANDIDATE_HOLDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "holder-candidate",
    construct: construct_fresh_holder,
    dependencies: no_dependencies,
    default: false,
}];

fn transient_seed_factories() -> &'static [ComponentFactoryDescriptor] {
    &TRANSIENT_SEED_FACTORY
}

fn active_holder_factories() -> &'static [ComponentFactoryDescriptor] {
    &ACTIVE_HOLDER_FACTORY
}

fn candidate_holder_factories() -> &'static [ComponentFactoryDescriptor] {
    &CANDIDATE_HOLDER_FACTORY
}

fn fresh_holder(factories: fn() -> &'static [ComponentFactoryDescriptor]) -> ComponentDescriptor {
    ComponentDescriptor {
        id: "fresh-holder",
        name: "FreshHolder",
        ty: TypeDescriptor::of::<FreshHolder>("FreshHolder"),
        scope: &Singleton,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn transient_seed() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "transient-seed",
        name: "TransientSeed",
        ty: TypeDescriptor::of::<TransientSeed>("TransientSeed"),
        scope: &Transient,
        condition: None,
        factories: transient_seed_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

#[tokio::test]
async fn candidate_registry_keeps_factory_backed_transients_fresh_resolvable() {
    let mut active = AppRegistry::default();
    active
        .components
        .extend([transient_seed(), fresh_holder(active_holder_factories)]);
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let mut candidate = AppRegistry::default();
    candidate
        .components
        .extend([transient_seed(), fresh_holder(candidate_holder_factories)]);
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "fresh-holder",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("holder reconstruction is supported");

    let (root, _) = resolved
        .build_candidate_root(&candidate, Vec::new(), ResolverSet::new())
        .await
        .expect("resolved plan constructs candidate root");
    let holder = root
        .resolve::<std::sync::Arc<FreshHolder>>()
        .await
        .expect("fresh holder resolves from the candidate root")
        .expect("fresh holder is stored in the candidate root");

    holder
        .fresh
        .create()
        .await
        .expect("factory-backed transient stays Fresh-resolvable after the transition");
}

// ---------------------------------------------------------------------------
// Retained candidate preparation from the pinned active root.
// ---------------------------------------------------------------------------

trait Svc: Send + Sync {
    fn label(&self) -> &'static str;
}

struct Retainable {
    label: &'static str,
}

impl Svc for Retainable {
    fn label(&self) -> &'static str {
        self.label
    }
}

impl Component for Retainable {
    type Handle = Arc<Self>;

    const ID: &'static str = "retainable";
    const NAME: &'static str = "Retainable";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// A framework-style seed descriptor: typed identity, no factory, snapshot adapter.
fn retainable_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual_of::<Retainable>("retainable", "Retainable", &Singleton)
}

fn erase_retainable_as_svc(boxed: &BoxedComponent) -> BoxedComponent {
    let live = boxed
        .value
        .downcast_ref::<Live<Retainable>>()
        .expect("retained slot holds a Live cell");
    let as_trait: Arc<dyn Svc> = live.snapshot();

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn Svc>("dyn Svc"),
        value: Box::new(Injectable::into_stored(as_trait)),
    }
}

fn svc_provider() -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Svc>("dyn Svc"),
        concrete_ty: TypeDescriptor::of::<Retainable>(Retainable::NAME),
        qualifier: "svc",
        primary: false,
        priority: 0,
        ordering: &[],
        erase: erase_retainable_as_svc,
    }
}

fn scope_registry(
    descriptors: &[ComponentDescriptor],
    providers: Vec<ProviderDescriptor>,
) -> Arc<ScopeRegistry> {
    let components = descriptors
        .iter()
        .map(|descriptor| (descriptor.ty.type_id, *descriptor))
        .collect();

    Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("scope registry validates"),
    )
}

fn seed_retainable(instance: &Arc<Retainable>) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<Retainable>(Retainable::NAME),
        value: Box::new(Injectable::into_stored(Arc::clone(instance))),
    }
}

fn seed_root_resolver() -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<RootResolver>(RootResolver::NAME),
        value: Box::new(Injectable::into_stored(RootResolver::new())),
    }
}

#[tokio::test]
async fn mixed_retain_and_reconstruct_builds_a_complete_candidate_root() {
    let mut active = AppRegistry::default();
    active.components.extend([
        retainable_descriptor(),
        replaceable(active_factories),
        root_resolver_descriptor(),
    ]);
    active.providers.push(svc_provider());
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");

    let active_instance = Arc::new(Retainable {
        label: "active-instance",
    });
    let active_hooks = HookManager::new(Vec::new());
    let active_scopes = scope_registry(&active.components, active.providers.clone());
    let active_root = ScopeContainer::build_root(
        &active.components,
        vec![
            seed_retainable(&active_instance),
            seed_root_resolver(),
            seed_hook_manager(&active_hooks),
        ],
        ResolverSet::new(),
        Arc::clone(&active_scopes),
    )
    .await
    .expect("active root builds");
    active_root
        .get::<RootResolver>()
        .expect("active resolver is seeded")
        .attach(&active_root);
    let foreign_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("foreign graph validates");
    let foreign = runtime_view(
        Arc::clone(&active_root),
        Arc::clone(&active_scopes),
        active.components.clone(),
        foreign_graph,
    );
    let active = runtime_view(
        Arc::clone(&active_root),
        active_scopes,
        active.components.clone(),
        active_graph,
    );

    let mut candidate = AppRegistry::default();
    candidate.components.extend([
        retainable_descriptor(),
        replaceable(candidate_factories),
        root_resolver_descriptor(),
    ]);
    candidate.providers.push(svc_provider());
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("mixed retain and reconstruct resolves");

    let Err(foreign_error) = resolved
        .build_candidate_root_from_active(&candidate, &foreign, ResolverSet::new())
        .await
    else {
        panic!("a foreign runtime view cannot supply retained instances");
    };

    assert!(matches!(foreign_error, Error::StaleGraphCandidate(_)));

    let (root, scopes) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
        .expect("retained candidate root builds");

    assert!(root.belongs_to_registry(&scopes));

    let retained = root
        .resolve::<Arc<Retainable>>()
        .await
        .expect("candidate root resolves")
        .expect("retained component is present");

    assert!(
        Arc::ptr_eq(&retained, &active_instance),
        "the retained singleton must share the active Arc instance"
    );

    let aliased = root
        .resolve::<Arc<dyn Svc>>()
        .await
        .expect("candidate root resolves the provider")
        .expect("candidate provider alias is present");

    assert_eq!(aliased.label(), "active-instance");
    assert_eq!(
        Arc::as_ptr(&aliased) as *const u8,
        Arc::as_ptr(&active_instance) as *const u8,
        "the candidate provider alias must be rebuilt from the retained concrete"
    );

    let reconstructed = root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("candidate root resolves")
        .expect("reconstructed component is present");

    assert_eq!(reconstructed.label, "candidate");

    let resolver = root
        .get::<RootResolver>()
        .expect("candidate resolver is recreated");
    let resolved_from_candidate = resolver
        .extract::<Arc<Replaceable>>()
        .await
        .expect("candidate resolver resolves the candidate root");

    assert_eq!(
        resolved_from_candidate.label, "candidate",
        "the root resolver must be recreated for the candidate generation"
    );

    let active_replaceable = active_root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("active root resolves")
        .expect("active replaceable is present");

    assert_eq!(
        active_replaceable.label, "active",
        "the active root is untouched by candidate preparation"
    );
}

#[tokio::test]
async fn raw_manual_singleton_requires_restart() {
    let user_provided = || {
        ComponentDescriptor::manual(
            "user-provided",
            "UserProvided",
            TypeDescriptor::of::<u8>("UserProvided"),
            &Singleton,
        )
    };
    let mut active = AppRegistry::default();
    active
        .components
        .extend([replaceable(active_factories), user_provided()]);
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let active_scopes = scope_registry(&active.components, Vec::new());
    let active_hooks = HookManager::new(Vec::new());
    let active_root = ScopeContainer::build_root(
        &active.components,
        vec![
            BoxedComponent {
                ty: TypeDescriptor::of::<u8>("UserProvided"),
                value: Box::new(8u8),
            },
            seed_hook_manager(&active_hooks),
        ],
        ResolverSet::new(),
        Arc::clone(&active_scopes),
    )
    .await
    .expect("active root builds");
    let active = runtime_view(
        Arc::clone(&active_root),
        active_scopes,
        active.components.clone(),
        active_graph,
    );

    let mut candidate = AppRegistry::default();
    candidate
        .components
        .extend([replaceable(candidate_factories), user_provided()]);
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("the unchanged manual component receives a retain disposition");

    let Err(error) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
    else {
        panic!("a raw manual singleton has no transition contract");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "user-provided",
            required: Some(NodeAction::Retain),
            reason: RestartReason::ManualInstanceUnsupported,
        })
    ));
}

#[derive(Clone)]
struct UnretainedByValue;

impl Component for UnretainedByValue {
    type Handle = Self;

    const ID: &'static str = "unretained-by-value";
    const NAME: &'static str = "UnretainedByValue";

    fn into_handle(self) -> Self {
        self
    }
}

impl Injectable for UnretainedByValue {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[tokio::test]
async fn typed_handle_without_snapshot_contract_requires_distinct_restart() {
    let unretained = || {
        ComponentDescriptor::manual_of::<UnretainedByValue>(
            UnretainedByValue::ID,
            UnretainedByValue::NAME,
            &Singleton,
        )
    };
    let mut active = AppRegistry::default();
    active
        .components
        .extend([replaceable(active_factories), unretained()]);
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let active_scopes = scope_registry(&active.components, Vec::new());
    let active_hooks = HookManager::new(Vec::new());
    let active_root = ScopeContainer::build_root(
        &active.components,
        vec![
            BoxedComponent {
                ty: unretained().ty,
                value: Box::new(UnretainedByValue),
            },
            seed_hook_manager(&active_hooks),
        ],
        ResolverSet::new(),
        Arc::clone(&active_scopes),
    )
    .await
    .expect("active root builds");
    let active = runtime_view(
        active_root,
        active_scopes,
        active.components.clone(),
        active_graph,
    );

    let mut candidate = AppRegistry::default();
    candidate
        .components
        .extend([replaceable(candidate_factories), unretained()]);
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("unchanged typed component receives a retain disposition");

    let Err(error) = resolved
        .build_candidate_root_from_active(&candidate, &active, ResolverSet::new())
        .await
    else {
        panic!("a typed handle without a snapshot contract must restart");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: "unretained-by-value",
            required: Some(NodeAction::Retain),
            reason: RestartReason::HandleRetentionUnsupported,
        })
    ));
}

// ---------------------------------------------------------------------------
// Generation override seeds for framework singletons.
// ---------------------------------------------------------------------------

/// The framework-seeded hook-manager descriptor: typed identity, no factory, no
/// snapshot adapter — generation-local by contract.
fn hook_manager_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        HOOK_MANAGER_ID,
        HOOK_MANAGER_NAME,
        TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        &Singleton,
    )
}

fn seed_hook_manager(hooks: &HookManager) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(hooks.clone())),
    }
}

/// A hook kind registered only on override managers, so an override manager is
/// distinguishable from the active generation's manager by catalog.
struct OverrideProbeKind;

impl HookKind for OverrideProbeKind {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "override-probe";
}

/// The probe hook's nominal receiver type; the call is never invoked.
struct OverrideProbeComponent;

fn override_probe_kind_ty() -> TypeId {
    TypeId::of::<OverrideProbeKind>()
}

fn override_probe_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

/// The boxed future shape of an erased hook call.
type OverrideCallFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

fn unreachable_override_call<'a>(
    _: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> OverrideCallFuture<'a> {
    Box::pin(async { unreachable!("override probe hooks are never invoked") })
}

fn override_probe_manager() -> HookManager {
    HookManager::new(vec![HookDescriptor::new(
        1,
        TypeDescriptor::of::<OverrideProbeComponent>("OverrideProbeComponent"),
        OverrideProbeKind::NAME,
        override_probe_kind_ty,
        override_probe_dependencies,
        unreachable_override_call,
    )])
}

/// An active root holding a snapshot-capable retained singleton, a reconstructed
/// singleton, and the framework hook-manager seed, plus the candidate plan that
/// reconstructs `replaceable`.
struct HookOverrideFixture {
    active: RuntimeView,
    active_root: Arc<ScopeContainer>,
    active_instance: Arc<Retainable>,
    candidate: CandidateGraph,
    resolved: ResolvedTransitionPlan,
}

async fn hook_override_fixture() -> HookOverrideFixture {
    let active_instance = Arc::new(Retainable {
        label: "active-instance",
    });
    let active_hooks = HookManager::new(Vec::new());

    let mut active = AppRegistry::default();
    active.components.extend([
        retainable_descriptor(),
        replaceable(active_factories),
        hook_manager_descriptor(),
    ]);
    let active_graph = EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &active.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates");
    let active_scopes = scope_registry(&active.components, Vec::new());
    let active_root = ScopeContainer::build_root(
        &active.components,
        vec![
            seed_retainable(&active_instance),
            seed_hook_manager(&active_hooks),
        ],
        ResolverSet::new(),
        Arc::clone(&active_scopes),
    )
    .await
    .expect("active root builds");
    let active = runtime_view(
        Arc::clone(&active_root),
        active_scopes,
        active.components.clone(),
        active_graph,
    );

    let mut candidate = AppRegistry::default();
    candidate.components.extend([
        retainable_descriptor(),
        replaceable(candidate_factories),
        hook_manager_descriptor(),
    ]);
    let candidate = CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &candidate, &topology())
        .expect("candidate graph validates");
    let resolved = candidate
        .resolve_transition(
            active.effective_graph(),
            [ComponentTransitionDecision {
                component: "replaceable",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("mixed retain and reconstruct resolves");

    HookOverrideFixture {
        active,
        active_root,
        active_instance,
        candidate,
        resolved,
    }
}

#[tokio::test]
async fn hook_manager_override_replaces_only_the_framework_seed() {
    let fixture = hook_override_fixture().await;

    let (root, scopes) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![seed_hook_manager(&override_probe_manager())],
        )
        .await
        .expect("overridden candidate root builds");

    assert!(root.belongs_to_registry(&scopes));

    let hooks = root
        .get::<HookManager>()
        .expect("candidate root stores a hook manager");

    assert!(
        hooks.has::<OverrideProbeKind>(),
        "the override manager must replace the retained framework seed"
    );

    let retained = root
        .resolve::<Arc<Retainable>>()
        .await
        .expect("candidate root resolves")
        .expect("retained component is present");

    assert!(
        Arc::ptr_eq(&retained, &fixture.active_instance),
        "the non-overridden retained singleton must remain snapshot-derived"
    );

    let reconstructed = root
        .resolve::<Arc<Replaceable>>()
        .await
        .expect("candidate root resolves")
        .expect("reconstructed component is present");

    assert_eq!(reconstructed.label, "candidate");

    let active_hooks = fixture
        .active_root
        .get::<HookManager>()
        .expect("active root stores its hook manager");

    assert!(
        !active_hooks.has::<OverrideProbeKind>(),
        "the active root's manager is untouched by candidate preparation"
    );
}

#[tokio::test]
async fn duplicate_generation_overrides_are_rejected() {
    let fixture = hook_override_fixture().await;

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![
                seed_hook_manager(&override_probe_manager()),
                seed_hook_manager(&override_probe_manager()),
            ],
        )
        .await
    else {
        panic!("duplicate generation overrides are invalid");
    };

    assert!(matches!(
        error,
        Error::DuplicateGenerationOverride {
            type_name: HOOK_MANAGER_NAME
        }
    ));
}

#[tokio::test]
async fn generation_overrides_must_replace_retained_singletons() {
    let fixture = hook_override_fixture().await;
    let reconstructed_override = BoxedComponent {
        ty: TypeDescriptor::of::<Replaceable>("Replaceable"),
        value: Box::new(Injectable::into_stored(Arc::new(Replaceable {
            label: "override",
        }))),
    };

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            vec![reconstructed_override],
        )
        .await
    else {
        panic!("an override for a reconstructed component is invalid");
    };

    assert!(matches!(
        error,
        Error::InvalidGenerationOverride {
            type_name: "Replaceable"
        }
    ));
}

#[tokio::test]
async fn unoverridden_hook_manager_retention_still_requires_restart() {
    let fixture = hook_override_fixture().await;

    let Err(error) = fixture
        .resolved
        .build_candidate_root_from_active_with_overrides(
            &fixture.candidate,
            &fixture.active,
            ResolverSet::new(),
            Vec::new(),
        )
        .await
    else {
        panic!("the hook manager has no snapshot contract");
    };

    assert!(matches!(
        error,
        Error::RestartRequired(super::RestartRequired {
            component: HOOK_MANAGER_ID,
            required: Some(NodeAction::Retain),
            reason: RestartReason::ManualInstanceUnsupported,
        })
    ));
}
