//! Pre-commit transition hooks for the provider-switch fixture: the affected
//! [`AuthGate`] consumer participates in both phases, and an unaffected [`AuditLog`]
//! carries a targeted participant that must never run, an `always` operator policy, and
//! `always` observers in both phases. Each hook records what it saw into [`seen`], and the
//! test drives each hook's verdict through [`set_controls`]. Tests sharing these statics
//! hold the fixture's test guard.

use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use upwell_config::CfgNext;
use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, Injectable, NodeAction, ReasonKind, Singleton,
};
use upwell_hooks::{HookDescriptor, HookKind, HookParam, HookRole, HookRouting};

use super::fixture::{AuthConfig, PrimaryAuthenticator, observed_modes};
use super::gate::{AuthGate, gate_factories};
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

type HookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

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

fn gate_proposal_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    let behavior = behavior(|controls| controls.gate_proposal);

    Box::pin(participate::<TransitionProposal>(
        "gate-proposal",
        cx,
        behavior,
    ))
}

fn gate_decision_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    let behavior = behavior(|controls| controls.gate_decision);

    Box::pin(participate::<TransitionDecision>(
        "gate-decision",
        cx,
        behavior,
    ))
}

fn audit_targeted_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(participate::<TransitionProposal>(
        "audit-targeted",
        cx,
        Behavior::Accept,
    ))
}

fn policy_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    let behavior = behavior(|controls| controls.policy);

    Box::pin(participate::<TransitionProposal>("policy", cx, behavior))
}

fn proposal_observer_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(observe::<TransitionProposal>("proposal-observer", cx))
}

fn decision_observer_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(observe::<TransitionDecision>("decision-observer", cx))
}

fn proposal_kind() -> TypeId {
    TypeId::of::<TransitionProposal>()
}

fn decision_kind() -> TypeId {
    TypeId::of::<TransitionDecision>()
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

const GATE_TY: TypeDescriptor = TypeDescriptor::of::<AuthGate>(AuthGate::NAME);
const AUDIT_TY: TypeDescriptor = TypeDescriptor::of::<AuditLog>(AuditLog::NAME);

static GATE_HOOKS: [HookDescriptor; 2] = [
    HookDescriptor::new(
        1,
        GATE_TY,
        TransitionProposal::NAME,
        proposal_kind,
        no_dependencies,
        gate_proposal_call,
    ),
    HookDescriptor::new(
        2,
        GATE_TY,
        TransitionDecision::NAME,
        decision_kind,
        no_dependencies,
        gate_decision_call,
    ),
];

static AUDIT_HOOKS: [HookDescriptor; 4] = [
    HookDescriptor::new(
        1,
        AUDIT_TY,
        TransitionProposal::NAME,
        proposal_kind,
        no_dependencies,
        audit_targeted_call,
    ),
    HookDescriptor::new(
        2,
        AUDIT_TY,
        TransitionProposal::NAME,
        proposal_kind,
        no_dependencies,
        policy_call,
    )
    .with_routing(HookRouting::Always),
    HookDescriptor::new(
        3,
        AUDIT_TY,
        TransitionProposal::NAME,
        proposal_kind,
        no_dependencies,
        proposal_observer_call,
    )
    .with_role(HookRole::Observer)
    .with_routing(HookRouting::Always),
    HookDescriptor::new(
        4,
        AUDIT_TY,
        TransitionDecision::NAME,
        decision_kind,
        no_dependencies,
        decision_observer_call,
    )
    .with_role(HookRole::Observer)
    .with_routing(HookRouting::Always),
];

fn gate_hooks() -> &'static [HookDescriptor] {
    &GATE_HOOKS
}

fn audit_hooks() -> &'static [HookDescriptor] {
    &AUDIT_HOOKS
}

/// The fixed `AuthGate` consumer with participant hooks in both pre-commit phases.
pub(super) static HOOKED_GATE_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: AuthGate::ID,
    name: AuthGate::NAME,
    ty: GATE_TY,
    scope: &Singleton,
    condition: None,
    factories: gate_factories,
    hooks: gate_hooks,
    generation_snapshot: None,
};

/// An operator-owned singleton no auth switch affects.
pub(super) struct AuditLog;

impl Component for AuditLog {
    type Handle = Arc<Self>;

    const ID: &'static str = "audit-log";
    const NAME: &'static str = "AuditLog";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

type AuditFactoryFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>>;

fn construct_audit(_context: &mut ComponentConstructionContext) -> AuditFactoryFuture<'_> {
    Box::pin(async move {
        Ok(BoxedComponent {
            ty: AUDIT_TY,
            value: Box::new(Injectable::into_stored(Arc::new(AuditLog))),
        })
    })
}

fn audit_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_audit,
        dependencies: no_dependencies,
        default: true,
    }]
}

/// The audit log's descriptor: unaffected by every auth switch, so it is retained across
/// generations through its typed snapshot adapter.
pub(super) static AUDIT_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: AuditLog::ID,
    name: AuditLog::NAME,
    ty: AUDIT_TY,
    scope: &Singleton,
    condition: None,
    factories: audit_factories,
    hooks: audit_hooks,
    generation_snapshot: ComponentDescriptor::typed_snapshot::<AuditLog>(),
};
