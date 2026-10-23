//! Transition outcome observers for the provider-switch fixture: an unaffected
//! [`OutcomeLog`] observes every committed and aborted attempt, recording the final report
//! it received into [`observed`]. [`set_outcome_controls`] makes the committed observer
//! fail, panic, or request a follow-up reload. Tests sharing these statics hold the
//! fixture's test guard.

use std::any::Any;
use std::sync::Mutex;

use upwell_core::TypeDescriptor;
use upwell_di::{Component, ComponentDescriptor, factories};
use upwell_hooks::{HookDescriptor, HookKind, HookParam};

use crate::test_support::{hook_call, test_component};
use crate::{
    AppRuntime, RuntimeTransitionReport, TransitionAborted, TransitionCommitted,
    TransitionOutcomeContext,
};

/// A panic payload that must never reach a trace.
pub(super) const SECRET_OBSERVER_PANIC: &str = "secret-observer-payload";

/// What the committed observer does after recording its report.
#[derive(Default)]
pub(super) enum OutcomeControls {
    #[default]
    Record,
    Fail,
    Panic,
    FollowUp(AppRuntime),
}

/// One observed outcome: which kind fired and the report it received, plus whether a
/// follow-up reload requested from the observer was rejected as reentrant.
#[derive(Clone)]
pub(super) struct Observed {
    pub(super) kind: &'static str,
    pub(super) report: RuntimeTransitionReport,
    pub(super) follow_up_reentrant: Option<bool>,
}

static CONTROLS: Mutex<Option<OutcomeControls>> = Mutex::new(None);
static OBSERVED: Mutex<Vec<Observed>> = Mutex::new(Vec::new());

pub(super) fn set_outcome_controls(controls: OutcomeControls) {
    *CONTROLS.lock().expect("controls") = Some(controls);
    OBSERVED.lock().expect("observed").clear();
}

pub(super) fn observed() -> Vec<Observed> {
    OBSERVED.lock().expect("observed").clone()
}

pub(super) fn observed_kinds() -> Vec<&'static str> {
    observed().iter().map(|observed| observed.kind).collect()
}

fn take_controls() -> OutcomeControls {
    CONTROLS
        .lock()
        .expect("controls")
        .replace(OutcomeControls::Record)
        .unwrap_or_default()
}

fn report_of<K>(cx: &(dyn Any + Send + Sync)) -> upwell_hooks::Result<RuntimeTransitionReport>
where
    K: HookKind<Cx = TransitionOutcomeContext>,
{
    let cx = cx
        .downcast_ref::<TransitionOutcomeContext>()
        .expect("transition outcome context");

    <RuntimeTransitionReport as HookParam<K>>::extract(cx, None)
}

fn record(kind: &'static str, report: RuntimeTransitionReport, follow_up: Option<bool>) {
    OBSERVED.lock().expect("observed").push(Observed {
        kind,
        report,
        follow_up_reentrant: follow_up,
    });
}

hook_call!(committed_call, |_ctx, cx| async move {
    let report = report_of::<TransitionCommitted>(cx)?;
    let controls = take_controls();
    let follow_up = match &controls {
        OutcomeControls::FollowUp(runtime) => Some(matches!(
            runtime.reload_config().await,
            Err(crate::Error::ReentrantReload { .. })
        )),
        _ => None,
    };

    record(TransitionCommitted::NAME, report, follow_up);

    match controls {
        OutcomeControls::Fail => Err(upwell_hooks::Error::MissingParam("audit sink")),
        OutcomeControls::Panic => panic!("{SECRET_OBSERVER_PANIC}"),
        OutcomeControls::Record | OutcomeControls::FollowUp(_) => {
            Ok(Box::new(()) as Box<dyn Any + Send>)
        }
    }
});

hook_call!(aborted_call, |_ctx, cx| async move {
    record(
        TransitionAborted::NAME,
        report_of::<TransitionAborted>(cx)?,
        None,
    );

    Ok(Box::new(()) as Box<dyn Any + Send>)
});

const OUTCOME_TY: TypeDescriptor = TypeDescriptor::of::<OutcomeLog>(OutcomeLog::NAME);

static OUTCOME_HOOKS: [HookDescriptor; 2] = [
    HookDescriptor::of::<TransitionCommitted>(1, OUTCOME_TY, committed_call),
    HookDescriptor::of::<TransitionAborted>(2, OUTCOME_TY, aborted_call),
];

/// An operator-owned singleton no auth switch affects, observing every outcome.
pub(super) struct OutcomeLog;

test_component!(OutcomeLog, "outcome-log");

async fn construct_outcome_log() -> OutcomeLog {
    OutcomeLog
}

/// The outcome log's descriptor, retained across generations through its typed snapshot
/// adapter.
pub(super) static OUTCOME_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<OutcomeLog>()
    .with_factories(factories!["static" => construct_outcome_log])
    .with_hooks(|| &OUTCOME_HOOKS);
