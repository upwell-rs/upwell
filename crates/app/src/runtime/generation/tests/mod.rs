use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, DescriptorSource, Resolver, ResolverCtx, ResolverCtxExt,
    ResolverSet, RuntimeGenerationId, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentDescriptor, ComponentRegistry, ConditionFactSnapshot, EffectiveGraph,
    Injectable, ScopeContainer, ScopeRegistry, Singleton,
};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookKind, HookManager};

use super::*;
use crate::test_support::{HookFuture, unbuilt_factories};
use crate::{AppRegistry, ScopeTopology};

const ENABLED: ConfigFactId = ConfigFactId::new("test::GenerationConfig", "generation", "enabled");
const SOURCE: DescriptorSource = upwell_core::descriptor_source!();

static GENERATION_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "generation-enabled",
    source: SOURCE,
    predicate: ConditionPredicate::ConfigBool(ENABLED),
};

struct GenerationComponent;

static GENERATION_COMPONENT: ComponentDescriptor = ComponentDescriptor::manual(
    "generation_component",
    "GenerationComponent",
    TypeDescriptor::of::<GenerationComponent>("GenerationComponent"),
    &Singleton,
)
.with_condition(&GENERATION_CONDITION)
.with_factories(unbuilt_factories!("static", Vec::new, true));

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

mod commit;
mod hooks;
mod publication;
