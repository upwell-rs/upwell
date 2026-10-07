//! Focused tests for the reload invalidation-root computation: the deterministic set
//! of candidate components a non-empty staged reload must force to reconstruct.
//!
//! The unit tests drive the private helper directly over prepared candidate graphs;
//! the async test proves the transactional reload wires the computed roots into the
//! runtime resolver, so a changed binding reconstructs its config consumer even when
//! the candidate graph is structurally identical to the active one.

use std::any::{Any, TypeId};
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use serde::Deserialize;
use upwell_config::{ConfigBinding, ConfigProperties, ConfigReload};
use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, ResolverCtx,
    RuntimeGenerationId, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    Singleton,
};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookDescriptor, HookKind, HookManager};

use super::super::invalidated_component_roots;
use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, factory_calls, lock_test_guard, reset_controls,
    write_probe_config,
};
use crate::transition::CandidateGraph;
use crate::{AppRegistry, ScopeTopology};

#[derive(Deserialize)]
struct ReloadConfig;

impl ConfigProperties for ReloadConfig {
    const NAME: &'static str = "ReloadConfig";
}

#[derive(Deserialize)]
struct OtherConfig;

impl ConfigProperties for OtherConfig {
    const NAME: &'static str = "OtherConfig";
}

/// Two distinct config types that share one display name and one binding path. Their
/// `TypeId`s — and only their `TypeId`s — tell them apart.
mod shared_left {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct SharedConfig;
}

mod shared_right {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct SharedConfig;
}

impl ConfigProperties for shared_left::SharedConfig {
    const NAME: &'static str = "SharedConfig";
}

impl ConfigProperties for shared_right::SharedConfig {
    const NAME: &'static str = "SharedConfig";
}

// ---------------------------------------------------------------------------
// Descriptor fixtures: one distinct marker type per component, because the
// registry collapses descriptors to one per type.
// ---------------------------------------------------------------------------

struct ConfigConsumer;

struct UnqualifiedConsumer;

struct OtherPathConsumer;

struct OtherTypeConsumer;

struct HookOwner;

struct ManagerConsumer;

struct Unrelated;

struct Dual;

struct AlsoConfig;

struct SharedLeftConsumer;

struct SharedRightConsumer;

fn unreachable_construct(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { unreachable!("invalidation tests never construct components") })
}

fn config_dependency<T: ConfigProperties>(
    qualifier: Option<&'static str>,
) -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: T::NAME,
        ty: TypeDescriptor::of::<T>(T::NAME),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier,
        config: true,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

fn hook_manager_dependency() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        name: HOOK_MANAGER_NAME,
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        cardinality: Cardinality::One,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution: ResolutionMode::Eager,
        observation: DependencyObservation::Snapshot,
    }]
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn qualified_probe_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<ReloadConfig>(Some("probe"))
}

fn unqualified_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<ReloadConfig>(None)
}

fn other_path_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<ReloadConfig>(Some("settings"))
}

fn other_type_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<OtherConfig>(Some("probe"))
}

fn shared_left_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<shared_left::SharedConfig>(Some("shared"))
}

fn shared_right_deps() -> Vec<DependencyDescriptor> {
    config_dependency::<shared_right::SharedConfig>(Some("shared"))
}

fn manager_deps() -> Vec<DependencyDescriptor> {
    hook_manager_dependency()
}

static QUALIFIED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: qualified_probe_deps,
    default: false,
}];

static UNQUALIFIED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: unqualified_deps,
    default: false,
}];

static OTHER_PATH_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: other_path_deps,
    default: false,
}];

static OTHER_TYPE_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: other_type_deps,
    default: false,
}];

static SHARED_LEFT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: shared_left_deps,
    default: false,
}];

static SHARED_RIGHT_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: shared_right_deps,
    default: false,
}];

static MANAGER_CONSUMER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: manager_deps,
    default: false,
}];

static PLAIN_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: unreachable_construct,
    dependencies: no_dependencies,
    default: false,
}];

fn qualified_factories() -> &'static [ComponentFactoryDescriptor] {
    &QUALIFIED_FACTORY
}

fn unqualified_factories() -> &'static [ComponentFactoryDescriptor] {
    &UNQUALIFIED_FACTORY
}

fn other_path_factories() -> &'static [ComponentFactoryDescriptor] {
    &OTHER_PATH_FACTORY
}

fn other_type_factories() -> &'static [ComponentFactoryDescriptor] {
    &OTHER_TYPE_FACTORY
}

fn shared_left_factories() -> &'static [ComponentFactoryDescriptor] {
    &SHARED_LEFT_FACTORY
}

fn shared_right_factories() -> &'static [ComponentFactoryDescriptor] {
    &SHARED_RIGHT_FACTORY
}

fn manager_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &MANAGER_CONSUMER_FACTORY
}

fn plain_factories() -> &'static [ComponentFactoryDescriptor] {
    &PLAIN_FACTORY
}

fn hook_owner_kind_ty() -> TypeId {
    TypeId::of::<ConfigReload>()
}

fn hook_owner_hook_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

type HookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

fn unreachable_hook_call<'a>(
    _: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async { unreachable!("invalidation tests never invoke hooks") })
}

static RELOAD_HOOK: HookDescriptor = HookDescriptor::new(
    1,
    TypeDescriptor::of::<HookOwner>("HookOwner"),
    <ConfigReload as HookKind>::NAME,
    hook_owner_kind_ty,
    hook_owner_hook_dependencies,
    unreachable_hook_call,
);

static RELOAD_HOOKS: [HookDescriptor; 1] = [RELOAD_HOOK];

fn reload_hook_hooks() -> &'static [HookDescriptor] {
    &RELOAD_HOOKS
}

fn singleton(
    id: &'static str,
    name: &'static str,
    ty: TypeDescriptor,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
    hooks: fn() -> &'static [HookDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name,
        ty,
        scope: &Singleton,
        condition: None,
        factories,
        hooks,
        generation_snapshot: None,
    }
}

fn config_consumer() -> ComponentDescriptor {
    singleton(
        "config-consumer",
        "ConfigConsumer",
        TypeDescriptor::of::<ConfigConsumer>("ConfigConsumer"),
        qualified_factories,
        upwell_hooks::no_hooks,
    )
}

fn unqualified_consumer() -> ComponentDescriptor {
    singleton(
        "unqualified-consumer",
        "UnqualifiedConsumer",
        TypeDescriptor::of::<UnqualifiedConsumer>("UnqualifiedConsumer"),
        unqualified_factories,
        upwell_hooks::no_hooks,
    )
}

fn other_path_consumer() -> ComponentDescriptor {
    singleton(
        "other-path-consumer",
        "OtherPathConsumer",
        TypeDescriptor::of::<OtherPathConsumer>("OtherPathConsumer"),
        other_path_factories,
        upwell_hooks::no_hooks,
    )
}

fn other_type_consumer() -> ComponentDescriptor {
    singleton(
        "other-type-consumer",
        "OtherTypeConsumer",
        TypeDescriptor::of::<OtherTypeConsumer>("OtherTypeConsumer"),
        other_type_factories,
        upwell_hooks::no_hooks,
    )
}

fn hook_owner() -> ComponentDescriptor {
    singleton(
        "hook-owner",
        "HookOwner",
        TypeDescriptor::of::<HookOwner>("HookOwner"),
        plain_factories,
        reload_hook_hooks,
    )
}

fn manager_consumer() -> ComponentDescriptor {
    singleton(
        "manager-consumer",
        "ManagerConsumer",
        TypeDescriptor::of::<ManagerConsumer>("ManagerConsumer"),
        manager_consumer_factories,
        upwell_hooks::no_hooks,
    )
}

fn unrelated() -> ComponentDescriptor {
    singleton(
        "unrelated",
        "Unrelated",
        TypeDescriptor::of::<Unrelated>("Unrelated"),
        plain_factories,
        upwell_hooks::no_hooks,
    )
}

fn dual() -> ComponentDescriptor {
    singleton(
        "dual",
        "Dual",
        TypeDescriptor::of::<Dual>("Dual"),
        qualified_factories,
        reload_hook_hooks,
    )
}

fn also_config() -> ComponentDescriptor {
    singleton(
        "also-config",
        "AlsoConfig",
        TypeDescriptor::of::<AlsoConfig>("AlsoConfig"),
        qualified_factories,
        upwell_hooks::no_hooks,
    )
}

fn shared_left_consumer() -> ComponentDescriptor {
    singleton(
        "shared-left-consumer",
        "SharedLeftConsumer",
        TypeDescriptor::of::<SharedLeftConsumer>("SharedLeftConsumer"),
        shared_left_factories,
        upwell_hooks::no_hooks,
    )
}

fn shared_right_consumer() -> ComponentDescriptor {
    singleton(
        "shared-right-consumer",
        "SharedRightConsumer",
        TypeDescriptor::of::<SharedRightConsumer>("SharedRightConsumer"),
        shared_right_factories,
        upwell_hooks::no_hooks,
    )
}

/// The framework-seeded hook-manager descriptor: typed identity, no factory —
/// generation-local by contract.
fn hook_manager_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        HOOK_MANAGER_ID,
        HOOK_MANAGER_NAME,
        TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        &Singleton,
    )
}

fn topology() -> crate::PreparedScopeTopology {
    ScopeTopology::empty()
        .prepare()
        .expect("empty topology validates")
}

fn candidate(components: &[ComponentDescriptor], bindings: Vec<ConfigBinding>) -> CandidateGraph {
    let mut registry = AppRegistry::default();
    registry.components.extend(components.iter().copied());
    registry.config_bindings.extend(bindings);

    CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &topology())
        .expect("candidate graph validates")
}

/// One changed binding's exact invalidation input for the config type `T` at `path`,
/// shaped as the reload derives them from the changed staged entries: the binding's
/// exact `TypeId` and its path.
fn changed<T: ConfigProperties + 'static>(path: &'static str) -> Vec<(TypeId, &'static str)> {
    vec![(TypeId::of::<T>(), path)]
}

// ---------------------------------------------------------------------------
// Changed config-binding matching.
// ---------------------------------------------------------------------------

#[test]
fn qualified_config_dependency_matches_changed_binding() {
    let candidate = candidate(
        &[config_consumer()],
        vec![ConfigBinding::of::<ReloadConfig>("probe")],
    );

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert_eq!(invalidated, BTreeSet::from(["config-consumer"]));
}

#[test]
fn unqualified_config_dependency_matches_changed_binding_by_type() {
    let candidate = candidate(
        &[unqualified_consumer()],
        vec![ConfigBinding::of::<ReloadConfig>("probe")],
    );

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert_eq!(invalidated, BTreeSet::from(["unqualified-consumer"]));
}

#[test]
fn same_path_different_type_does_not_match() {
    let candidate = candidate(
        &[other_type_consumer()],
        vec![ConfigBinding::of::<OtherConfig>("probe")],
    );

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert!(
        invalidated.is_empty(),
        "a changed binding of another type must not invalidate a same-path consumer"
    );
}

#[test]
fn qualified_path_mismatch_does_not_match() {
    let candidate = candidate(
        &[other_path_consumer()],
        vec![
            ConfigBinding::of::<ReloadConfig>("probe"),
            ConfigBinding::of::<ReloadConfig>("settings"),
        ],
    );

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert!(
        invalidated.is_empty(),
        "a qualified dependency must match only its exact binding path"
    );
}

#[test]
fn same_display_name_and_path_only_invalidates_the_changed_type() {
    let candidate = candidate(
        &[shared_left_consumer(), shared_right_consumer()],
        vec![
            ConfigBinding::of::<shared_left::SharedConfig>("shared"),
            ConfigBinding::of::<shared_right::SharedConfig>("shared"),
        ],
    );

    let invalidated =
        invalidated_component_roots(&candidate, &changed::<shared_left::SharedConfig>("shared"));

    assert_eq!(
        invalidated,
        BTreeSet::from(["shared-left-consumer"]),
        "only the changed binding's type invalidates; a distinct type sharing the display \
         name and binding path must not"
    );
}

// ---------------------------------------------------------------------------
// Hook owners and HookManager consumers.
// ---------------------------------------------------------------------------

#[test]
fn config_reload_hook_owner_is_invalidated_without_factory_config_dependency() {
    let candidate = candidate(&[hook_owner()], Vec::new());

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert_eq!(
        invalidated,
        BTreeSet::from(["hook-owner"]),
        "a ConfigReload hook owner is conservatively invalidated on any changed reload"
    );
}

#[test]
fn hook_manager_consumer_is_invalidated() {
    let candidate = candidate(&[hook_manager_descriptor(), manager_consumer()], Vec::new());

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert_eq!(
        invalidated,
        BTreeSet::from(["manager-consumer"]),
        "the HookManager consumer reconstructs; the generation-local manager seed does not"
    );
}

#[test]
fn unrelated_components_are_excluded() {
    let candidate = candidate(&[unrelated()], Vec::new());

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert!(invalidated.is_empty(), "an unrelated component stays put");
}

#[test]
fn overlapping_criteria_deduplicate_deterministically() {
    let candidate = candidate(
        &[dual(), also_config()],
        vec![ConfigBinding::of::<ReloadConfig>("probe")],
    );

    let invalidated = invalidated_component_roots(&candidate, &changed::<ReloadConfig>("probe"));

    assert_eq!(
        invalidated,
        BTreeSet::from(["also-config", "dual"]),
        "a component matching several criteria appears once, in deterministic order"
    );
}

// ---------------------------------------------------------------------------
// Wiring: the transactional reload feeds the computed roots to the resolver.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn changed_binding_reconstructs_its_config_consumer() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    // The probe is active and the graph is structurally identical after the reload;
    // only its binding's value changed.
    assert!(runtime.root().get::<ProbeComponent>().is_some());
    let factories_before = factory_calls();

    write_probe_config(&config_dir_of(&dir), true, 7);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(report.changed.len(), 1, "only the probe binding changed");

    assert_eq!(
        factory_calls(),
        factories_before + 1,
        "the changed binding forces the config consumer to reconstruct"
    );

    let probe = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the probe resolves from the new root");

    assert_eq!(
        probe.observed, 7,
        "the reconstructed factory resolved the staged proposed value"
    );
}
