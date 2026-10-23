//! Component retirement hooks for the provider-switch fixture: the primary
//! [`PrimaryAuthenticator`] and the fixed [`AuthGate`] consumer each retire themselves, and
//! an unaffected [`RetirementAudit`] carries a targeted retirement hook that must never run
//! plus an `always` hook that observes every retirement. Each hook records the retirement
//! it received and the address of the receiver it resolved into [`retired`], and
//! [`set_retirement_controls`] makes one owner's hook fail or panic. Tests sharing these
//! statics hold the fixture's test guard.

use std::any::Any;
use std::sync::{Arc, Mutex};

use upwell_core::{ResolverCtx, TypeDescriptor};
use upwell_di::{Component, ComponentDescriptor, factories};
use upwell_hooks::{HookDescriptor, HookParam, HookRouting};

use super::fixture::{PrimaryAuthenticator, primary_component};
use super::gate::{AuthGate, GATE_FACTORIES};
use crate::test_support::{HookFuture, hook_call, hook_receiver, test_component};
use crate::{ComponentRetired, ComponentRetirement, RetirementContext};

/// A panic payload that must never reach a trace.
pub(super) const SECRET_RETIREMENT_PANIC: &str = "secret-retirement-payload";

/// The owner the audit's `always` hook records under.
pub(super) const AUDIT_ALWAYS: &str = "retirement-audit/always";

/// Which owners' retirement hooks fail or panic after recording.
#[derive(Clone, Copy, Default)]
pub(super) struct RetirementControls {
    pub(super) failing: Option<&'static str>,
    pub(super) panicking: Option<&'static str>,
}

/// One retirement hook run: its owner, the retirement it received, and the address of the
/// receiver instance it resolved.
#[derive(Clone, Copy, Debug)]
pub(super) struct Retired {
    pub(super) owner: &'static str,
    pub(super) retirement: ComponentRetirement,
    pub(super) receiver: usize,
}

static CONTROLS: Mutex<RetirementControls> = Mutex::new(RetirementControls {
    failing: None,
    panicking: None,
});
static RETIRED: Mutex<Vec<Retired>> = Mutex::new(Vec::new());

pub(super) fn set_retirement_controls(controls: RetirementControls) {
    *CONTROLS.lock().expect("controls") = controls;
    RETIRED.lock().expect("retired").clear();
}

pub(super) fn retired() -> Vec<Retired> {
    RETIRED.lock().expect("retired").clone()
}

/// The retirements `owner`'s hooks received, in firing order.
pub(super) fn retired_by(owner: &str) -> Vec<Retired> {
    retired()
        .into_iter()
        .filter(|retired| retired.owner == owner)
        .collect()
}

/// Resolves the `C` receiver the way the `#[hook]` macro does, records the retirement
/// under `owner`, then applies the owner's controls.
async fn retire_as<C>(
    owner: &'static str,
    ctx: &(dyn ResolverCtx + Send + Sync),
    cx: &(dyn Any + Send + Sync),
) -> upwell_hooks::Result<Box<dyn Any + Send>>
where
    C: Component<Handle = Arc<C>> + Send + Sync,
{
    let cx = cx
        .downcast_ref::<RetirementContext>()
        .expect("retirement context");
    let retirement = <ComponentRetirement as HookParam<ComponentRetired>>::extract(cx, None)?;
    let receiver = hook_receiver::<C>(ctx)?;
    let controls = *CONTROLS.lock().expect("controls");

    RETIRED.lock().expect("retired").push(Retired {
        owner,
        retirement,
        receiver: Arc::as_ptr(&receiver) as usize,
    });

    if controls.panicking == Some(owner) {
        panic!("{SECRET_RETIREMENT_PANIC}");
    }

    if controls.failing == Some(owner) {
        return Err(upwell_hooks::Error::MissingParam("retirement sink"));
    }

    Ok(Box::new(()) as Box<dyn Any + Send>)
}

fn retire_call<'a, C>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a>
where
    C: Component<Handle = Arc<C>> + Send + Sync,
{
    Box::pin(retire_as::<C>(C::ID, ctx, cx))
}

hook_call!(audit_always_call, |ctx, cx| {
    retire_as::<RetirementAudit>(AUDIT_ALWAYS, ctx, cx)
});

const fn retirement_hook(
    ordinal: u32,
    owner: TypeDescriptor,
    call: upwell_hooks::HookCall,
) -> HookDescriptor {
    HookDescriptor::of::<ComponentRetired>(ordinal, owner, call)
}

const PRIMARY_TY: TypeDescriptor =
    TypeDescriptor::of::<PrimaryAuthenticator>(PrimaryAuthenticator::NAME);
const GATE_TY: TypeDescriptor = TypeDescriptor::of::<AuthGate>(AuthGate::NAME);
const AUDIT_TY: TypeDescriptor = TypeDescriptor::of::<RetirementAudit>(RetirementAudit::NAME);

static PRIMARY_HOOKS: [HookDescriptor; 1] = [retirement_hook(
    1,
    PRIMARY_TY,
    retire_call::<PrimaryAuthenticator>,
)];
static GATE_HOOKS: [HookDescriptor; 1] = [retirement_hook(1, GATE_TY, retire_call::<AuthGate>)];
static AUDIT_HOOKS: [HookDescriptor; 2] = [
    retirement_hook(1, AUDIT_TY, retire_call::<RetirementAudit>),
    retirement_hook(2, AUDIT_TY, audit_always_call).with_routing(HookRouting::Always),
];

/// The primary provider, retiring itself when a switch removes it.
pub(super) static RETIRING_PRIMARY_COMPONENT: ComponentDescriptor =
    primary_component(|| &PRIMARY_HOOKS);

/// The fixed gate consumer, retiring itself when a switch reconstructs it.
pub(super) static RETIRING_GATE_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<AuthGate>()
        .with_factories(GATE_FACTORIES)
        .with_hooks(|| &GATE_HOOKS)
        .without_retention();

/// An operator-owned singleton no auth switch affects, auditing every retirement.
pub(super) struct RetirementAudit;

test_component!(RetirementAudit, "retirement-audit");

async fn construct_audit() -> RetirementAudit {
    RetirementAudit
}

/// The retirement audit's descriptor, retained across generations through its typed
/// snapshot adapter.
pub(super) static AUDIT_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<RetirementAudit>()
        .with_factories(factories!["static" => construct_audit])
        .with_hooks(|| &AUDIT_HOOKS);
