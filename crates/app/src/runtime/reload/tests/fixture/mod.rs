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
//! [`lock_test_guard`](super::lock_test_guard) serializes the tests themselves under the
//! default `cargo test` parallelism because they share the counters and controls.

use std::any::Any;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{
    Cfg, CfgNext, ConditionFacts, ConfigManager, ConfigProperties, ConfigReload, HookOutcome,
    ReloadProposal, ResolverChain, Toml,
};
use upwell_core::{
    ConditionCallback, ConditionContext, ConditionDescriptor, ConditionInputId, ConditionPredicate,
    ConditionPurity, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
    DependencyDescriptor, TypeDescriptor,
};
use upwell_di::{Component, ComponentDescriptor, factories};
use upwell_hooks::{HookDescriptor, HookKind, HookManager, HookParam};

use crate::test_support::{hook_call, hook_receiver, test_component};
use crate::{App, AppBuilder};

mod controls;

pub(super) use controls::*;

const PROBE_ENABLED: ConfigFactId = ConfigFactId::new("test::ProbeConfig", "probe", "enabled");

/// The probe's availability callback: its only declared input is the `probe.enabled`
/// fact, and its default outcome matches the `ConfigBool` predicate it replaces — the
/// fact must be present and true. Under test control it panics instead, so a test can
/// prove the reload's incremental evaluation is panic-contained.
fn probe_condition_evaluate(context: ConditionContext<'_>) -> bool {
    if CONDITION_PANIC.load(Ordering::SeqCst) {
        panic!("config condition callback panicked");
    }

    context
        .fact(PROBE_ENABLED)
        .and_then(ConditionScalar::as_bool)
        .unwrap_or(false)
}

static PROBE_CONDITION_CALLBACK: ConditionCallback = ConditionCallback {
    kind: "test/probe-enabled",
    inputs: &[ConditionInputId::Fact(PROBE_ENABLED)],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: probe_condition_evaluate,
};

static PROBE_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "probe-enabled",
    source: upwell_core::DescriptorSource::caller(),
    predicate: ConditionPredicate::Callback(&PROBE_CONDITION_CALLBACK),
};

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
        if DESCRIPTORS_PANIC.load(Ordering::SeqCst) {
            panic!("condition fact descriptor callback panicked");
        }

        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::ProbeConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::DescriptorSource::caller(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        if SCALARS_PANIC.load(Ordering::SeqCst) {
            panic!("condition fact extraction panicked");
        }

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

test_component!(ProbeComponent, "probe_component");

async fn construct_probe_component(cfg: Cfg<ProbeConfig>) -> Result<ProbeComponent, &'static str> {
    let observed = cfg.snapshot().token;

    FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
    FACTORY_TOKENS.lock().await.push(observed);

    if *FACTORY_FAILS.lock().await {
        return Err("probe candidate factory failed after resolving the staged config");
    }

    if *FACTORY_WAITS.lock().await {
        FACTORY_PARKED.notify_one();
        FACTORY_RELEASE.notified().await;
    }

    Ok(ProbeComponent {
        observed,
        cfg,
        mutated: AtomicBool::new(false),
    })
}

fn probe_reload_deps() -> Vec<DependencyDescriptor> {
    vec![<CfgNext<ProbeConfig> as HookParam<ConfigReload>>::dependency(None)]
}

hook_call!(probe_reload_call, |ctx, cx| async move {
    let proposal = cx
        .downcast_ref::<ReloadProposal>()
        .expect("config reload hook context");
    let receiver = hook_receiver::<ProbeComponent>(ctx)?;
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
});

/// A test-only hook kind a test fires explicitly through a manager of its choice —
/// never by the reload pipeline — to show which receiver that manager's own resolver
/// context resolves. The context carries nothing: the signal is the resolved receiver.
pub(super) struct ManagerProbeKind;

impl HookKind for ManagerProbeKind {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "manager_probe";
}

hook_call!(probe_manager_probe_call, |ctx, _cx| async move {
    let receiver = hook_receiver::<ProbeComponent>(ctx)?;

    MANAGER_PROBE_TOKENS.lock().await.push(receiver.observed);
    MANAGER_PROBE_IDS
        .lock()
        .await
        .push(Arc::as_ptr(&receiver) as usize);

    Ok(Box::new(()) as Box<dyn Any + Send>)
});

const PROBE_TY: TypeDescriptor = TypeDescriptor::of::<ProbeComponent>(ProbeComponent::NAME);

static PROBE_HOOKS: [HookDescriptor; 2] = [
    HookDescriptor::of::<ConfigReload>(1, PROBE_TY, probe_reload_call)
        .with_dependencies(probe_reload_deps),
    HookDescriptor::of::<ManagerProbeKind>(2, PROBE_TY, probe_manager_probe_call),
];

/// Fires the test-only [`ManagerProbeKind`] through `hooks` — the caller-chosen
/// manager, typically a [`ManagerConsumer`]'s stored one — and returns the single
/// hook outcome, so a test can distinguish a resolved run from a
/// [`ResolverUnavailable`](upwell_hooks::Error::ResolverUnavailable) one.
pub(super) async fn fire_manager_probe(hooks: &HookManager) -> upwell_hooks::Result<()> {
    let outcomes = hooks.fire_all::<ManagerProbeKind>(&()).await.participants;

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

static PROBE_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<ProbeComponent>()
    .with_condition(&PROBE_CONDITION)
    .with_factories(factories![default "static" => construct_probe_component])
    .with_hooks(|| &PROBE_HOOKS)
    .without_retention();

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

test_component!(ManagerConsumer, "manager_consumer");

async fn construct_manager_consumer(hooks: HookManager) -> ManagerConsumer {
    ManagerConsumer { hooks }
}

static MANAGER_CONSUMER_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<ManagerConsumer>()
        .with_factories(factories![default "static" => construct_manager_consumer])
        .without_retention();

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

pub(super) use super::config_dir_of;

pub(super) fn write_probe_config(config_dir: &Path, enabled: bool, token: i64) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[probe]\nenabled = {enabled}\ntoken = {token}\n"),
    )
    .expect("write probe config");
}
