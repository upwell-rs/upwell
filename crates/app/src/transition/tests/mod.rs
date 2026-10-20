use std::any::Any;
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_config::{ConfigBinding, ConfigManager, ConfigProperties, ConfigReloader};
use upwell_core::{
    ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId, ResolverCtx,
    ResolverSet, RuntimeGenerationId, Transient, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, EffectiveGraph, EffectiveNodeRole, Fresh,
    FromContainer, Injectable, NodeAction, PlannedNode, ProviderDescriptor, RootResolver,
    ScopeContainer, ScopeRegistry, Singleton, root_resolver_descriptor,
};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookDescriptor, HookKind, HookManager};

use super::{
    CandidateGraph, ComponentTransitionDecision, ComponentTransitionStrategy,
    ResolvedTransitionPlan, RestartReason, RestartRequired,
};
use crate::runtime::{AppConditionState, AppRuntime, PreparedRuntimeGeneration, RuntimeScopePlan};
use crate::test_support::test_component;
use crate::{AppRegistry, Error, RuntimeView, ScopeTopology};
use fixtures::*;

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

fn no_dependencies() -> Vec<upwell_core::DependencyDescriptor> {
    Vec::new()
}

/// Declares a unit test component `$ty` with stable ID `$id` and a non-retainable
/// descriptor fn `$descriptor()` whose one factory, also named `$id`, takes `$dep`s as
/// its parameters, providing each trait object after `provides`.
macro_rules! unit_component {
    (
        $ty:ident, $id:literal, $descriptor:ident
        $(, deps [$($dep:ty),*])? $(, provides $($provided:ty),+)?
    ) => {
        pub(super) struct $ty;

        test_component!($ty, $id $(, provides $($provided),+)?);

        pub(super) fn $descriptor() -> ComponentDescriptor {
            async fn construct($($(_: $dep),*)?) -> $ty {
                $ty
            }

            ComponentDescriptor::of::<$ty>()
                .with_factories(upwell_di::factories![$id => construct])
                .without_retention()
        }
    };
}

/// A registry of `components` and `providers`.
fn registry(
    components: impl IntoIterator<Item = ComponentDescriptor>,
    providers: impl IntoIterator<Item = ProviderDescriptor>,
) -> AppRegistry {
    let mut registry = AppRegistry::default();

    registry.components.extend(components);
    registry.providers.extend(providers);

    registry
}

/// The active graph and candidate prepared from one registry of `components`, so only
/// runtime invalidations can force transition work.
fn unchanged_graphs(
    components: impl IntoIterator<Item = ComponentDescriptor>,
) -> (EffectiveGraph, CandidateGraph) {
    let registry = registry(components, []);

    graphs(&registry, &registry)
}

/// The active graph built from `active` and the candidate prepared from `candidate`.
fn graphs(active: &AppRegistry, candidate: &AppRegistry) -> (EffectiveGraph, CandidateGraph) {
    (active_graph(active), candidate_graph(candidate))
}

fn active_graph(registry: &AppRegistry) -> EffectiveGraph {
    EffectiveGraph::build(
        RuntimeGenerationId::INITIAL,
        &registry.component_registry(),
        |_, _| true,
    )
    .expect("active graph validates")
}

fn candidate_graph(registry: &AppRegistry) -> CandidateGraph {
    CandidateGraph::prepare(RuntimeGenerationId::INITIAL, registry, &topology())
        .expect("candidate graph validates")
}

/// Builds the active root of `registry` from its pre-built `seeds`, attaching a seeded
/// root resolver to it, and returns the root with its scope registry.
async fn active_root(
    registry: &AppRegistry,
    seeds: Vec<BoxedComponent>,
) -> (Arc<ScopeContainer>, Arc<ScopeRegistry>) {
    let scopes = scope_registry(&registry.components, registry.providers.clone());
    let root = ScopeContainer::build_root(
        &registry.components,
        seeds,
        ResolverSet::new(),
        Arc::clone(&scopes),
    )
    .await
    .expect("active root builds");

    if let Some(resolver) = root.get::<RootResolver>() {
        resolver.attach(&root);
    }

    (root, scopes)
}

/// The pinned view of the active generation [`active_root`] builds for `registry`.
async fn active_view(registry: &AppRegistry, seeds: Vec<BoxedComponent>) -> RuntimeView {
    let (root, scopes) = active_root(registry, seeds).await;

    runtime_view(
        root,
        scopes,
        registry.components.clone(),
        active_graph(registry),
    )
}

mod evaluation;
mod fixtures;
mod fresh;
mod invalidation;
mod live_invalidation;
mod live_resolver;
mod live_retarget;
mod overrides;
mod recipes;
mod replacement;
mod retention;
mod strategy;
