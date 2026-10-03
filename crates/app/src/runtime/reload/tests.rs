//! Tests for the transactional config-and-graph reload entry point.
//!
//! Each test drives [`AppRuntime::reload_config`] over a file-backed app with one
//! condition-gated component whose factory injects `Cfg<T>` and whose `ConfigReload`
//! hook records the proposed value, so the tests can prove which store each stage of
//! the transaction resolved through.

use std::any::{Any, TypeId};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{
    Cfg, CfgNext, ConditionFacts, ConfigManager, ConfigProperties, ConfigReload, HookOutcome,
    ReloadProposal, ResolverChain, Toml,
};
use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, DependencyDescriptor, ResolverCtx, TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, FromContainer, Injectable, Singleton,
};
use upwell_hooks::{HookDescriptor, HookKind, HookParam};

use crate::App;
const PROBE_ENABLED: ConfigFactId = ConfigFactId::new("test::ProbeConfig", "probe", "enabled");

static PROBE_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "probe-enabled",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigBool(PROBE_ENABLED),
};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROPOSED_TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Serializes the reload tests: they share the factory/hook/proposal counters, so
/// concurrent runs under the default `cargo test` parallelism must not interleave.
/// Held for the entire body of each async test; await-safe, so no std guard ever
/// crosses an await and no sleeps are needed.
static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn lock_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_GUARD.lock().await
}

fn reset_counters() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    HOOK_CALLS.store(0, Ordering::SeqCst);
    PROPOSED_TOKENS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

fn factory_calls() -> usize {
    FACTORY_CALLS.load(Ordering::SeqCst)
}

fn hook_calls() -> usize {
    HOOK_CALLS.load(Ordering::SeqCst)
}

fn proposed_tokens() -> Vec<i64> {
    PROPOSED_TOKENS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[derive(Deserialize)]
struct ProbeConfig {
    enabled: bool,
    token: i64,
}

impl ConfigProperties for ProbeConfig {
    const NAME: &'static str = "ProbeConfig";
}

impl ConditionFacts for ProbeConfig {
    fn condition_facts() -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: PROBE_ENABLED,
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(PROBE_ENABLED, ConditionScalar::Bool(self.enabled))]
    }
}

/// Factory-backed singleton whose availability keys on the `probe.enabled` fact. The
/// factory records the config token it resolved from its generation's store, so a test
/// can tell whether construction saw the staged proposal or the active value.
struct ProbeComponent {
    observed: i64,
}

impl Component for ProbeComponent {
    type Handle = Arc<Self>;

    const ID: &'static str = "probe_component";
    const NAME: &'static str = "ProbeComponent";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn probe_dependencies() -> Vec<DependencyDescriptor> {
    vec![<Cfg<ProbeConfig> as FromContainer>::dependency()]
}

fn construct_probe_component(
    context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async move {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

        let cfg = <Cfg<ProbeConfig> as FromContainer>::from_container(context).await?;
        let observed = cfg.snapshot().token;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(ProbeComponent {
                observed,
            }))),
        })
    })
}

fn probe_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_probe_component,
        dependencies: probe_dependencies,
        default: true,
    }]
}

fn probe_reload_deps() -> Vec<DependencyDescriptor> {
    vec![<CfgNext<ProbeConfig> as HookParam<ConfigReload>>::dependency(None)]
}

fn probe_reload_kind_ty() -> TypeId {
    TypeId::of::<ConfigReload>()
}

/// The boxed hook future the erased [`upwell_hooks::HookCall`] signature returns.
type ProbeHookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

fn probe_reload_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> ProbeHookFuture<'a> {
    Box::pin(async move {
        let proposal = cx
            .downcast_ref::<ReloadProposal>()
            .expect("config reload hook context");

        let next = <CfgNext<ProbeConfig> as HookParam<ConfigReload>>::extract(proposal, None)?;

        PROPOSED_TOKENS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(next.token);
        HOOK_CALLS.fetch_add(1, Ordering::SeqCst);

        Ok(Box::new(HookOutcome::Unchanged) as Box<dyn Any + Send>)
    })
}

static PROBE_RELOAD_HOOK: HookDescriptor = HookDescriptor::new(
    1,
    TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME),
    <ConfigReload as HookKind>::NAME,
    probe_reload_kind_ty,
    probe_reload_deps,
    probe_reload_call,
);

static PROBE_HOOKS: [HookDescriptor; 1] = [PROBE_RELOAD_HOOK];

fn probe_hooks() -> &'static [HookDescriptor] {
    &PROBE_HOOKS
}

static PROBE_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ProbeComponent::ID,
    name: ProbeComponent::NAME,
    ty: TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME),
    scope: &Singleton,
    condition: Some(&PROBE_CONDITION),
    factories: probe_factories,
    hooks: probe_hooks,
    generation_snapshot: None,
};

/// Builds a file-backed app with the probe component registered, its config bound at
/// `probe`, and its availability fact sourced from the same binding. The temp dir is
/// returned so the source file outlives the app and can be rewritten by the test.
async fn build_probe_app(enabled: bool, token: i64) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-runtime-reload-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = config_dir_of(&dir);

    write_probe_config(&config_dir, enabled, token);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let app = App::<()>::builder("runtime-reload-test")
        .config_source(manager)
        .config::<ProbeConfig>("probe")
        .condition_facts::<ProbeConfig>("probe")
        .component_descriptor(&PROBE_COMPONENT)
        .build()
        .await
        .expect("probe app builds");

    (dir, app)
}

fn config_dir_of(dir: &TempDir) -> PathBuf {
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");

    config_dir
}

fn write_probe_config(config_dir: &Path, enabled: bool, token: i64) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[probe]\nenabled = {enabled}\ntoken = {token}\n"),
    )
    .expect("write probe config");
}

#[tokio::test]
async fn candidate_factory_resolves_proposed_config_before_publication() {
    let _guard = lock_test_guard().await;

    reset_counters();

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    // Initially disabled: the component is absent from the active generation and the
    // factory has never run.
    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();

    assert!(old_root.get::<ProbeComponent>().is_none());
    assert_eq!(factory_calls(), 0);

    write_probe_config(&config_dir_of(&dir), true, 7);

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(report.changed.len(), 1, "only the probe binding changed");
    assert_eq!(report.changed[0].path, "probe");
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the runtime generation advances exactly once"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation + 1,
        "the config generation advances once, at the terminal commit"
    );

    // The new root resolves the activated component, and its factory resolved the
    // staged proposed value through the candidate store: at construction time the
    // terminal commit had not run, so the active store still held the old token.
    let probe = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the activated component resolves from the new root");

    assert_eq!(
        probe.observed, 7,
        "the factory saw the staged proposed value"
    );
    assert_eq!(factory_calls(), 1, "the candidate factory ran exactly once");

    // The old generation's root is untouched by the transaction.
    assert!(old_root.get::<ProbeComponent>().is_none());

    // The reload hook ran against the candidate manager over the staged proposal.
    assert_eq!(report.hooks.len(), 1, "the config-reload hook ran");
    assert_eq!(
        proposed_tokens(),
        vec![7],
        "the hook observed the proposed value, not the active value"
    );
}

#[tokio::test]
async fn unchanged_reload_skips_candidate_work() {
    let _guard = lock_test_guard().await;

    reset_counters();

    let (_dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    let old_root = runtime.root();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();
    let factories_before = factory_calls();
    let hooks_before = hook_calls();

    assert!(
        runtime.hooks().has::<ConfigReload>(),
        "the probe hook is registered, so a zero hook count is meaningful"
    );

    // No source edit: the reload must be a true no-op.
    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(!report.published, "an unchanged source publishes nothing");
    assert!(report.changed.is_empty());
    assert!(report.hooks.is_empty());
    assert_eq!(
        runtime.generation(),
        old_generation,
        "the runtime generation is unchanged"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation,
        "the config generation is unchanged"
    );
    assert!(
        Arc::ptr_eq(&old_root, &runtime.root()),
        "the root identity is unchanged"
    );
    assert_eq!(
        factory_calls(),
        factories_before,
        "no candidate construction ran"
    );
    assert_eq!(hook_calls(), hooks_before, "no config-reload hook ran");
}
