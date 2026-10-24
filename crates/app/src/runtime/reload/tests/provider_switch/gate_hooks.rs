//! Pre-commit transition hooks for the provider-switch fixture: the affected
//! [`AuthGate`] consumer participates in both phases, and an unaffected [`AuditLog`]
//! carries a targeted participant that must never run, an `always` operator policy, and
//! `always` observers in both phases. Each hook records what it saw into [`seen`], and the
//! test drives each hook's verdict through [`set_controls`]. Tests sharing these statics
//! hold the fixture's test guard.

use std::any::Any;
use std::sync::Mutex;

use upwell_config::CfgNext;
use upwell_core::TypeDescriptor;
use upwell_di::{Component, ComponentDescriptor, NodeAction, ReasonKind, factories};
use upwell_hooks::{HookDescriptor, HookKind, HookParam, HookRole, HookRouting};

use super::fixture::{AuthConfig, PrimaryAuthenticator, observed_modes};
use super::gate::{AuthGate, GATE_FACTORIES};
use crate::test_support::{hook_call, test_component};
use crate::{
    AppRuntime, PendingTransition, TransitionDecision, TransitionHookContext, TransitionPhase,
    TransitionProposal, TransitionVerdict,
};

/// A panic payload that must never reach a report, error, or trace.
pub(super) const SECRET_PANIC: &str = "secret-panic-payload";

/// What one participant hook does once it recorded what it saw.
#[derive(Clone, Copy, Debug)]
pub(super) enum Behavior {
    Accept,
    Reject(&'static str),
    Panic,
}

/// The per-test behavior of every hook.
pub(super) struct Controls {
    pub(super) gate_proposal: Behavior,
    pub(super) gate_decision: Behavior,
    pub(super) policy: Behavior,
    pub(super) observer_panics: bool,
    pub(super) reload_from_proposal: Option<AppRuntime>,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            gate_proposal: Behavior::Accept,
            gate_decision: Behavior::Accept,
            policy: Behavior::Accept,
            observer_panics: false,
            reload_from_proposal: None,
        }
    }
}

/// What one hook run observed about the pending transition.
#[derive(Clone, Debug)]
pub(super) struct Seen {
    pub(super) hook: &'static str,
    pub(super) phase: TransitionPhase,
    pub(super) components: Vec<(&'static str, NodeAction)>,
    pub(super) primary_reasons: Vec<ReasonKind>,
    pub(super) changed: Vec<String>,
    pub(super) staged_mode: String,
    pub(super) constructions: usize,
    pub(super) config_hooks: usize,
    pub(super) nested_reload_rejected: Option<bool>,
}

static CONTROLS: Mutex<Option<Controls>> = Mutex::new(None);
static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

pub(super) fn set_controls(controls: Controls) {
    *CONTROLS.lock().expect("controls") = Some(controls);
    SEEN.lock().expect("seen").clear();
}

pub(super) fn seen() -> Vec<Seen> {
    SEEN.lock().expect("seen").clone()
}

pub(super) fn seen_hooks() -> Vec<&'static str> {
    seen().iter().map(|seen| seen.hook).collect()
}

fn behavior(select: impl FnOnce(&Controls) -> Behavior) -> Behavior {
    CONTROLS
        .lock()
        .expect("controls")
        .as_ref()
        .map_or(Behavior::Accept, select)
}

fn take_nested_reload() -> Option<AppRuntime> {
    CONTROLS
        .lock()
        .expect("controls")
        .as_mut()
        .and_then(|controls| controls.reload_from_proposal.take())
}

fn observer_panics() -> bool {
    CONTROLS
        .lock()
        .expect("controls")
        .as_ref()
        .is_some_and(|controls| controls.observer_panics)
}

/// Records what the hook saw through the same `HookParam` extraction the `#[hook]` macro
/// generates.
async fn record<K>(hook: &'static str, cx: &(dyn Any + Send + Sync)) -> upwell_hooks::Result<()>
where
    K: HookKind<Cx = TransitionHookContext>,
    CfgNext<AuthConfig>: HookParam<K>,
{
    let cx = cx
        .downcast_ref::<TransitionHookContext>()
        .expect("transition hook context");
    let pending = <PendingTransition as HookParam<K>>::extract(cx, None)?;
    let staged = <CfgNext<AuthConfig> as HookParam<K>>::extract(cx, Some("auth"))?;
    let primary_reasons = pending
        .component(PrimaryAuthenticator::ID)
        .map(|component| component.reasons.iter().map(|reason| reason.kind).collect())
        .unwrap_or_default();
    let constructions = observed_modes().await.len();

    SEEN.lock().expect("seen").push(Seen {
        hook,
        phase: pending.phase(),
        components: pending
            .components()
            .iter()
            .map(|component| (component.component, component.action))
            .collect(),
        primary_reasons,
        changed: pending
            .changed()
            .iter()
            .map(|binding| binding.path.clone())
            .collect(),
        staged_mode: staged.mode.clone(),
        constructions,
        config_hooks: pending.config_hooks().len(),
        nested_reload_rejected: None,
    });

    Ok(())
}

async fn participate<K>(
    hook: &'static str,
    cx: &(dyn Any + Send + Sync),
    behavior: Behavior,
) -> upwell_hooks::Result<Box<dyn Any + Send>>
where
    K: HookKind<Cx = TransitionHookContext>,
    CfgNext<AuthConfig>: HookParam<K>,
{
    record::<K>(hook, cx).await?;

    if let Some(runtime) = take_nested_reload() {
        let rejected = matches!(
            runtime.reload_config().await,
            Err(crate::Error::ReentrantReload { .. })
        );

        if let Some(seen) = SEEN.lock().expect("seen").last_mut() {
            seen.nested_reload_rejected = Some(rejected);
        }
    }

    let verdict = match behavior {
        Behavior::Accept => TransitionVerdict::Accept,
        Behavior::Reject(reason) => TransitionVerdict::Reject(reason),
        Behavior::Panic => panic!("{SECRET_PANIC}"),
    };

    Ok(Box::new(verdict))
}

async fn observe<K>(
    hook: &'static str,
    cx: &(dyn Any + Send + Sync),
) -> upwell_hooks::Result<Box<dyn Any + Send>>
where
    K: HookKind<Cx = TransitionHookContext>,
    CfgNext<AuthConfig>: HookParam<K>,
{
    record::<K>(hook, cx).await?;

    if observer_panics() && K::NAME == TransitionDecision::NAME {
        panic!("{SECRET_PANIC}");
    }

    Ok(Box::new(()))
}

hook_call!(gate_proposal_call, |_ctx, cx| {
    let behavior = behavior(|controls| controls.gate_proposal);

    participate::<TransitionProposal>("gate-proposal", cx, behavior)
});

hook_call!(gate_decision_call, |_ctx, cx| {
    let behavior = behavior(|controls| controls.gate_decision);

    participate::<TransitionDecision>("gate-decision", cx, behavior)
});

hook_call!(audit_targeted_call, |_ctx, cx| {
    participate::<TransitionProposal>("audit-targeted", cx, Behavior::Accept)
});

hook_call!(policy_call, |_ctx, cx| {
    let behavior = behavior(|controls| controls.policy);

    participate::<TransitionProposal>("policy", cx, behavior)
});

hook_call!(proposal_observer_call, |_ctx, cx| {
    observe::<TransitionProposal>("proposal-observer", cx)
});

hook_call!(decision_observer_call, |_ctx, cx| {
    observe::<TransitionDecision>("decision-observer", cx)
});

const GATE_TY: TypeDescriptor = TypeDescriptor::of::<AuthGate>(AuthGate::NAME);
const AUDIT_TY: TypeDescriptor = TypeDescriptor::of::<AuditLog>(AuditLog::NAME);

static GATE_HOOKS: [HookDescriptor; 2] = [
    HookDescriptor::of::<TransitionProposal>(1, GATE_TY, gate_proposal_call),
    HookDescriptor::of::<TransitionDecision>(2, GATE_TY, gate_decision_call),
];

static AUDIT_HOOKS: [HookDescriptor; 4] = [
    HookDescriptor::of::<TransitionProposal>(1, AUDIT_TY, audit_targeted_call),
    HookDescriptor::of::<TransitionProposal>(2, AUDIT_TY, policy_call)
        .with_routing(HookRouting::Always),
    HookDescriptor::of::<TransitionProposal>(3, AUDIT_TY, proposal_observer_call)
        .with_role(HookRole::Observer)
        .with_routing(HookRouting::Always),
    HookDescriptor::of::<TransitionDecision>(4, AUDIT_TY, decision_observer_call)
        .with_role(HookRole::Observer)
        .with_routing(HookRouting::Always),
];

/// The fixed `AuthGate` consumer with participant hooks in both pre-commit phases.
pub(super) static HOOKED_GATE_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<AuthGate>()
        .with_factories(GATE_FACTORIES)
        .with_hooks(|| &GATE_HOOKS)
        .without_retention();

/// An operator-owned singleton no auth switch affects.
pub(super) struct AuditLog;

test_component!(AuditLog, "audit-log");

async fn construct_audit() -> AuditLog {
    AuditLog
}

/// The audit log's descriptor: unaffected by every auth switch, so it is retained across
/// generations through its typed snapshot adapter.
pub(super) static AUDIT_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<AuditLog>()
    .with_factories(factories!["static" => construct_audit])
    .with_hooks(|| &AUDIT_HOOKS);
