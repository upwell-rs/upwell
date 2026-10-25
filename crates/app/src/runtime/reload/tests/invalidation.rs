//! Focused tests for the reload invalidation-root computation: the deterministic set
//! of candidate components a non-empty staged reload must force to reconstruct.
//!
//! The unit tests drive the private helper directly over prepared candidate graphs;
//! the async test proves the transactional reload wires the computed roots into the
//! runtime resolver, so a changed binding reconstructs its config consumer even when
//! the candidate graph is structurally identical to the active one.

use std::any::TypeId;
use std::collections::BTreeSet;

use serde::Deserialize;
use upwell_config::{ConfigBinding, ConfigProperties, ConfigReload};
use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, RuntimeGenerationId,
    Scope, TypeDescriptor,
};
use upwell_di::{ComponentDescriptor, Singleton};
use upwell_hooks::{HOOK_MANAGER_NAME, HookDescriptor, HookManager};

use super::super::invalidation::invalidated_component_roots;
use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, factory_calls, lock_test_guard, reset_controls,
    write_probe_config,
};
use crate::AppRegistry;
use crate::test_support::{
    empty_topology, hook_manager_descriptor, unbuilt_factories, uncalled_hook,
};
use crate::transition::CandidateGraph;

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

/// Declares marker `$ty` and `$descriptor()`: a singleton identified `$id` and named
/// after `$ty`, whose never-driven `"static"` recipe declares `$deps` and which carries
/// `$hooks` (none by default).
macro_rules! planned_component {
    ($ty:ident, $id:literal, $descriptor:ident, $deps:expr $(, hooks $hooks:expr)?) => {
        struct $ty;

        fn $descriptor() -> ComponentDescriptor {
            ComponentDescriptor {
                id: $id,
                name: stringify!($ty),
                ty: TypeDescriptor::of::<$ty>(stringify!($ty)),
                scope: Singleton::KEY,
                condition: None,
                factories: unbuilt_factories!("static", $deps),
                hooks: planned_component!(@hooks $($hooks)?),
                generation_snapshot: None,
            }
        }
    };
    (@hooks) => {
        upwell_hooks::no_hooks
    };
    (@hooks $hooks:expr) => {
        $hooks
    };
}

static RELOAD_HOOKS: [HookDescriptor; 1] = [HookDescriptor::of::<ConfigReload>(
    1,
    TypeDescriptor::of::<HookOwner>("HookOwner"),
    uncalled_hook,
)];

planned_component!(ConfigConsumer, "config-consumer", config_consumer, || {
    config_dependency::<ReloadConfig>(Some("probe"))
});
planned_component!(
    UnqualifiedConsumer,
    "unqualified-consumer",
    unqualified_consumer,
    || config_dependency::<ReloadConfig>(None)
);
planned_component!(
    OtherPathConsumer,
    "other-path-consumer",
    other_path_consumer,
    || { config_dependency::<ReloadConfig>(Some("settings")) }
);
planned_component!(
    OtherTypeConsumer,
    "other-type-consumer",
    other_type_consumer,
    || { config_dependency::<OtherConfig>(Some("probe")) }
);
planned_component!(
    HookOwner,
    "hook-owner",
    hook_owner,
    Vec::new,
    hooks || &RELOAD_HOOKS
);
planned_component!(
    ManagerConsumer,
    "manager-consumer",
    manager_consumer,
    hook_manager_dependency
);
planned_component!(Unrelated, "unrelated", unrelated, Vec::new);
planned_component!(
    Dual,
    "dual",
    dual,
    || config_dependency::<ReloadConfig>(Some("probe")),
    hooks || &RELOAD_HOOKS
);
planned_component!(AlsoConfig, "also-config", also_config, || {
    config_dependency::<ReloadConfig>(Some("probe"))
});
planned_component!(
    SharedLeftConsumer,
    "shared-left-consumer",
    shared_left_consumer,
    || config_dependency::<shared_left::SharedConfig>(Some("shared"))
);
planned_component!(
    SharedRightConsumer,
    "shared-right-consumer",
    shared_right_consumer,
    || config_dependency::<shared_right::SharedConfig>(Some("shared"))
);

fn candidate(components: &[ComponentDescriptor], bindings: Vec<ConfigBinding>) -> CandidateGraph {
    let mut registry = AppRegistry::default();
    registry.components.extend(components.iter().copied());
    registry.config_bindings.extend(bindings);

    CandidateGraph::prepare(RuntimeGenerationId::INITIAL, &registry, &empty_topology())
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

    assert!(
        report.is_committed(),
        "a changed source publishes a generation"
    );
    assert_eq!(report.changed().len(), 1, "only the probe binding changed");

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
