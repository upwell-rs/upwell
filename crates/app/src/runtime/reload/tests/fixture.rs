//! Shared fixture for the transactional reload tests: a file-backed app with one
//! condition-gated probe component whose factory injects `Cfg<T>` and whose
//! `ConfigReload` hook resolves its `&self` receiver through the resolver context (as
//! the `#[hook]` macro generates) and records the proposed value plus the receiver it
//! resolved — its observed token and process-local identity. The hook can also be told
//! to mutate its resolved receiver before returning, so a test can prove a rejected
//! candidate run never touches the active receiver.
//!
//! The probe also carries a test-only [`ManagerProbeKind`] hook: a test fires it
//! explicitly through a manager of its choice — typically the [`HookManager`] a
//! [`ManagerConsumer`] singleton stored at construction — and the run records which
//! receiver (token and identity) that manager's own resolver context resolved, so a
//! test can tell which generation's root a stored manager is still bound to.
//!
//! The factory and hook honor shared controls so a test can inject a failure or a
//! cancellation point at a specific stage of the transaction. Cross-await controls are
//! tokio-guarded; the synchronous config-deserialization log uses a `std::sync::Mutex`.
//! [`TEST_GUARD`] serializes the tests themselves under the default `cargo test`
//! parallelism because they share the counters and controls.

use std::any::{Any, TypeId};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{
    Cfg, CfgNext, ConditionFacts, ConfigManager, ConfigProperties, ConfigReload, HookOutcome,
    ReloadProposal, ResolverChain, Toml,
};
use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, DependencyDescriptor, ResolverCtx, ResolverCtxExt,
    TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentSource, FromContainer, Injectable, Singleton,
};
use upwell_hooks::{HookDescriptor, HookKind, HookManager, HookParam};

use crate::{App, AppBuilder};

const PROBE_ENABLED: ConfigFactId = ConfigFactId::new("test::ProbeConfig", "probe", "enabled");

static PROBE_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "probe-enabled",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigBool(PROBE_ENABLED),
};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
static STAGE_ENTRIES: AtomicUsize = AtomicUsize::new(0);

static PROPOSED_TOKENS: tokio::sync::Mutex<Vec<i64>> = tokio::sync::Mutex::const_new(Vec::new());
/// The `observed` token of the receiver each hook run resolved from its manager's
/// resolver context — the generation whose root the run resolved through.
static RECEIVER_TOKENS: tokio::sync::Mutex<Vec<i64>> = tokio::sync::Mutex::const_new(Vec::new());
/// The process-local identity (`Arc::as_ptr`) of the receiver each hook run resolved
/// from its manager's resolver context, in run order.
static RECEIVER_IDS: tokio::sync::Mutex<Vec<usize>> = tokio::sync::Mutex::const_new(Vec::new());
static FACTORY_TOKENS: tokio::sync::Mutex<Vec<i64>> = tokio::sync::Mutex::const_new(Vec::new());
static STAGED_TOKENS: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

/// The proposed token the config-reload hook rejects. `None` accepts every proposal.
static HOOK_REJECT_TOKEN: tokio::sync::Mutex<Option<i64>> = tokio::sync::Mutex::const_new(None);

/// Whether the config-reload hook mutates its resolved receiver before returning, so a
/// test can prove a rejected candidate run leaves the active receiver untouched.
static HOOK_MUTATES_RECEIVER: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// The identity and token of the receiver each hook run actually mutated, recorded only
/// after the mutation is applied, in run order.
static MUTATED_RECEIVERS: tokio::sync::Mutex<Vec<(usize, i64)>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// The observed token of the receiver each [`ManagerProbeKind`] run resolved through
/// the fired manager's own resolver context, in run order.
static MANAGER_PROBE_TOKENS: tokio::sync::Mutex<Vec<i64>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// The process-local identity (`Arc::as_ptr`) of the receiver each
/// [`ManagerProbeKind`] run resolved, in run order.
static MANAGER_PROBE_IDS: tokio::sync::Mutex<Vec<usize>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// Whether the candidate factory returns a DI error after resolving the staged value.
static FACTORY_FAILS: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Whether the candidate factory parks on [`FACTORY_RELEASE`] after resolving the
/// staged value, so a test can cancel the reload mid-transaction.
static FACTORY_WAITS: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Signaled by the factory once it is about to park on [`FACTORY_RELEASE`].
static FACTORY_PARKED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The gate the factory parks on while the test decides whether to cancel the reload.
static FACTORY_RELEASE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Serializes the reload tests: they share the factory/hook counters and the failure
/// controls, so concurrent runs under the default `cargo test` parallelism must not
/// interleave. Held for the entire body of each async test; await-safe, so no std
/// guard ever crosses an await and no sleeps are needed.
static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) async fn lock_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_GUARD.lock().await
}

/// Resets every counter and failure control. Call at the start of each test while
/// holding [`lock_test_guard`].
pub(super) async fn reset_controls() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    HOOK_CALLS.store(0, Ordering::SeqCst);
    STAGE_ENTRIES.store(0, Ordering::SeqCst);

    *PROPOSED_TOKENS.lock().await = Vec::new();
    *RECEIVER_TOKENS.lock().await = Vec::new();
    *RECEIVER_IDS.lock().await = Vec::new();
    *FACTORY_TOKENS.lock().await = Vec::new();
    STAGED_TOKENS
        .lock()
        .expect("staged token log is available")
        .clear();
    *HOOK_REJECT_TOKEN.lock().await = None;
    *HOOK_MUTATES_RECEIVER.lock().await = false;
    MUTATED_RECEIVERS.lock().await.clear();
    *MANAGER_PROBE_TOKENS.lock().await = Vec::new();
    *MANAGER_PROBE_IDS.lock().await = Vec::new();
    *FACTORY_FAILS.lock().await = false;
    *FACTORY_WAITS.lock().await = false;
}

pub(super) fn factory_calls() -> usize {
    FACTORY_CALLS.load(Ordering::SeqCst)
}

/// Returns how many reload attempts entered config staging.
pub(super) fn stage_entries() -> usize {
    STAGE_ENTRIES.load(Ordering::SeqCst)
}

pub(super) fn hook_calls() -> usize {
    HOOK_CALLS.load(Ordering::SeqCst)
}

pub(super) async fn proposed_tokens() -> Vec<i64> {
    PROPOSED_TOKENS.lock().await.clone()
}

/// The receiver token each hook run resolved, in run order.
pub(super) async fn receiver_tokens() -> Vec<i64> {
    RECEIVER_TOKENS.lock().await.clone()
}

/// The process-local identity of the receiver each hook run resolved, in run order.
pub(super) async fn receiver_ids() -> Vec<usize> {
    RECEIVER_IDS.lock().await.clone()
}

pub(super) async fn factory_tokens() -> Vec<i64> {
    FACTORY_TOKENS.lock().await.clone()
}

/// Returns the candidate config tokens deserialized during reload staging.
pub(super) fn staged_tokens() -> Vec<i64> {
    STAGED_TOKENS
        .lock()
        .expect("staged token log is available")
        .clone()
}

/// Makes the config-reload hook reject the proposal whose token is `token`.
pub(super) async fn reject_hook_token(token: i64) {
    *HOOK_REJECT_TOKEN.lock().await = Some(token);
}

/// Makes the config-reload hook mutate its resolved receiver before returning.
pub(super) async fn set_hook_mutates_receiver(mutates: bool) {
    *HOOK_MUTATES_RECEIVER.lock().await = mutates;
}

/// The identity and token of the receiver each hook run mutated, in run order.
pub(super) async fn mutated_receivers() -> Vec<(usize, i64)> {
    MUTATED_RECEIVERS.lock().await.clone()
}

/// The receiver token each [`ManagerProbeKind`] run resolved, in run order.
pub(super) async fn manager_probe_tokens() -> Vec<i64> {
    MANAGER_PROBE_TOKENS.lock().await.clone()
}

/// The process-local identity of the receiver each [`ManagerProbeKind`] run resolved,
/// in run order.
pub(super) async fn manager_probe_ids() -> Vec<usize> {
    MANAGER_PROBE_IDS.lock().await.clone()
}

/// Makes the candidate factory fail with a DI error after resolving the staged value.
pub(super) async fn fail_factory_after_staging(fails: bool) {
    *FACTORY_FAILS.lock().await = fails;
}

/// Makes the candidate factory park after resolving the staged value, until the reload
/// task is cancelled.
pub(super) async fn park_factory_after_staging(waits: bool) {
    *FACTORY_WAITS.lock().await = waits;
}

/// Waits until the factory is parked inside candidate construction.
pub(super) async fn factory_parked() {
    FACTORY_PARKED.notified().await;
}

/// Releases one factory blocked by [`park_factory_after_staging`].
pub(super) fn release_factory() {
    FACTORY_RELEASE.notify_one();
}

pub(super) struct ProbeConfig {
    #[allow(dead_code, reason = "the condition fact reads the enabled field")]
    enabled: bool,
    pub(super) token: i64,
}

#[derive(Deserialize)]
struct ProbeConfigInput {
    enabled: bool,
    token: i64,
}

impl<'de> Deserialize<'de> for ProbeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = ProbeConfigInput::deserialize(deserializer)?;

        STAGE_ENTRIES.fetch_add(1, Ordering::SeqCst);
        STAGED_TOKENS
            .lock()
            .expect("staged token log is available")
            .push(input.token);

        Ok(Self {
            enabled: input.enabled,
            token: input.token,
        })
    }
}

impl ConfigProperties for ProbeConfig {
    const NAME: &'static str = "ProbeConfig";
}

impl ConditionFacts for ProbeConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::ProbeConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("test::ProbeConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

/// Factory-backed singleton whose availability keys on the `probe.enabled` fact. The
/// factory records the config token it resolved from its generation's store, so a test
/// can tell whether construction saw the staged proposal or the active value.
pub(super) struct ProbeComponent {
    pub(super) observed: i64,
    /// The config handle the factory resolved, stored so a test can read through the
    /// consumer's own generation-local cell after a reload.
    pub(super) cfg: Cfg<ProbeConfig>,
    /// Receiver-local marker a hook run may mutate. The active instance must never
    /// observe a candidate hook run's mutation.
    pub(super) mutated: AtomicBool,
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

        FACTORY_TOKENS.lock().await.push(observed);

        if *FACTORY_FAILS.lock().await {
            return Err(upwell_di::Error::Other(
                "probe candidate factory failed after resolving the staged config".into(),
            ));
        }

        if *FACTORY_WAITS.lock().await {
            FACTORY_PARKED.notify_one();
            FACTORY_RELEASE.notified().await;
        }

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(ProbeComponent {
                observed,
                cfg,
                mutated: AtomicBool::new(false),
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
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> ProbeHookFuture<'a> {
    Box::pin(async move {
        let proposal = cx
            .downcast_ref::<ReloadProposal>()
            .expect("config reload hook context");

        // Resolve the `&self` receiver through the resolver context, exactly as the
        // `#[hook]` macro generates: the receiver comes from the root the manager is
        // attached to, so a run against a stale manager cannot resolve the current
        // instance.
        let receiver = ctx
            .get_resolver::<ComponentSource>()
            .and_then(|source| source.component::<ProbeComponent>())
            .ok_or(upwell_hooks::Error::MissingReceiver(ProbeComponent::NAME))?;

        let next = <CfgNext<ProbeConfig> as HookParam<ConfigReload>>::extract(proposal, None)?;

        PROPOSED_TOKENS.lock().await.push(next.token);
        RECEIVER_TOKENS.lock().await.push(receiver.observed);
        RECEIVER_IDS
            .lock()
            .await
            .push(Arc::as_ptr(&receiver) as usize);
        HOOK_CALLS.fetch_add(1, Ordering::SeqCst);

        if *HOOK_MUTATES_RECEIVER.lock().await {
            receiver.mutated.store(true, Ordering::SeqCst);

            // Recorded only after the mutation is applied, so an entry proves the
            // receiver at this identity and token was actually mutated.
            MUTATED_RECEIVERS
                .lock()
                .await
                .push((Arc::as_ptr(&receiver) as usize, receiver.observed));
        }

        if *HOOK_REJECT_TOKEN.lock().await == Some(next.token) {
            return Err(upwell_hooks::Error::Other(
                format!("rejecting the proposed token {}", next.token).into(),
            ));
        }

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

/// A test-only hook kind a test fires explicitly through a manager of its choice —
/// never by the reload pipeline — to show which receiver that manager's own resolver
/// context resolves. The context carries nothing: the signal is the resolved receiver.
pub(super) struct ManagerProbeKind;

impl HookKind for ManagerProbeKind {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "manager_probe";
}

fn probe_manager_probe_deps() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn probe_manager_probe_kind_ty() -> TypeId {
    TypeId::of::<ManagerProbeKind>()
}

fn probe_manager_probe_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> ProbeHookFuture<'a> {
    Box::pin(async move {
        // Resolve the `&self` receiver through the fired manager's own resolver
        // context, exactly as the `#[hook]` macro generates: the receiver comes from
        // the root the manager is attached to, so the run shows which generation's
        // instance that manager still resolves.
        let receiver = ctx
            .get_resolver::<ComponentSource>()
            .and_then(|source| source.component::<ProbeComponent>())
            .ok_or(upwell_hooks::Error::MissingReceiver(ProbeComponent::NAME))?;

        MANAGER_PROBE_TOKENS.lock().await.push(receiver.observed);
        MANAGER_PROBE_IDS
            .lock()
            .await
            .push(Arc::as_ptr(&receiver) as usize);

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

static PROBE_MANAGER_PROBE_HOOK: HookDescriptor = HookDescriptor::new(
    2,
    TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME),
    <ManagerProbeKind as HookKind>::NAME,
    probe_manager_probe_kind_ty,
    probe_manager_probe_deps,
    probe_manager_probe_call,
);

static PROBE_HOOKS: [HookDescriptor; 2] = [PROBE_RELOAD_HOOK, PROBE_MANAGER_PROBE_HOOK];

fn probe_hooks() -> &'static [HookDescriptor] {
    &PROBE_HOOKS
}

/// Fires the test-only [`ManagerProbeKind`] through `hooks` — the caller-chosen
/// manager, typically a [`ManagerConsumer`]'s stored one — and returns the single
/// hook outcome, so a test can distinguish a resolved run from a
/// [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) one.
pub(super) async fn fire_manager_probe(hooks: &HookManager) -> upwell_hooks::Result<()> {
    let outcomes = hooks.run::<ManagerProbeKind>(&(), |_| true).await;

    assert_eq!(
        outcomes.len(),
        1,
        "exactly one manager-probe hook is registered"
    );

    outcomes
        .into_iter()
        .next()
        .expect("one manager-probe outcome")
        .1
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

/// A factory-backed singleton that injects the generation's [`HookManager`] through
/// ordinary DI and stores it — the holder pattern of a component that fires hooks
/// later from its own stored handle. Because it consumes the framework hook-manager
/// singleton, every publication reconstructs it, so the current root's holder stores
/// the current generation's manager while an older generation's retained holder keeps
/// its own.
pub(super) struct ManagerConsumer {
    /// The manager this construction's generation injected — attached to that
    /// generation's root only.
    pub(super) hooks: HookManager,
}

impl Component for ManagerConsumer {
    type Handle = Arc<Self>;

    const ID: &'static str = "manager_consumer";
    const NAME: &'static str = "ManagerConsumer";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn manager_consumer_dependencies() -> Vec<DependencyDescriptor> {
    vec![<HookManager as FromContainer>::dependency()]
}

fn construct_manager_consumer(
    context: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async move {
        let hooks = <HookManager as FromContainer>::from_container(context).await?;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<ManagerConsumer>(ManagerConsumer::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(ManagerConsumer { hooks }))),
        })
    })
}

fn manager_consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_manager_consumer,
        dependencies: manager_consumer_dependencies,
        default: true,
    }]
}

static MANAGER_CONSUMER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: ManagerConsumer::ID,
    name: ManagerConsumer::NAME,
    ty: TypeDescriptor::of::<ManagerConsumer>(ManagerConsumer::NAME),
    scope: &Singleton,
    condition: None,
    factories: manager_consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

/// Builds a file-backed app with the probe component registered, its config bound at
/// `probe`, and its availability fact sourced from the same binding. The temp dir is
/// returned so the source file outlives the app and can be rewritten by the test.
pub(super) async fn build_probe_app(enabled: bool, token: i64) -> (TempDir, App<()>) {
    build_probe_app_with(enabled, token, |builder| builder).await
}

/// Builds the probe app plus the [`ManagerConsumer`] singleton, so a test can fire
/// hooks through a component-stored manager.
pub(super) async fn build_probe_app_with_manager_consumer(
    enabled: bool,
    token: i64,
) -> (TempDir, App<()>) {
    build_probe_app_with(enabled, token, |builder| {
        builder.component_descriptor(&MANAGER_CONSUMER_COMPONENT)
    })
    .await
}

/// Shared probe-app builder: `extend` adds the caller's extra registrations on top of
/// the probe component's.
async fn build_probe_app_with(
    enabled: bool,
    token: i64,
    extend: impl FnOnce(AppBuilder<()>) -> AppBuilder<()>,
) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-runtime-reload-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = config_dir_of(&dir);

    write_probe_config(&config_dir, enabled, token);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let builder = App::<()>::builder("runtime-reload-test")
        .config_source(manager)
        .config::<ProbeConfig>("probe")
        .condition_facts::<ProbeConfig>("probe")
        .component_descriptor(&PROBE_COMPONENT);

    let app = extend(builder).build().await.expect("probe app builds");

    (dir, app)
}

pub(super) fn config_dir_of(dir: &TempDir) -> PathBuf {
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");

    config_dir
}

pub(super) fn write_probe_config(config_dir: &Path, enabled: bool, token: i64) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[probe]\nenabled = {enabled}\ntoken = {token}\n"),
    )
    .expect("write probe config");
}
