//! Transition outcome observers for the provider-switch fixture: an unaffected
//! [`OutcomeLog`] observes every committed and aborted attempt, recording the final report
//! it received into [`observed`]. [`set_outcome_controls`] makes the committed observer
//! fail, panic, or request a follow-up reload. Tests sharing these statics hold the
//! fixture's test guard.

use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, Injectable, Singleton,
};
use upwell_hooks::{HookDescriptor, HookKind, HookParam};

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

type HookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

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

fn committed_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async move {
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
    })
}

fn aborted_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    cx: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    Box::pin(async move {
        record(
            TransitionAborted::NAME,
            report_of::<TransitionAborted>(cx)?,
            None,
        );

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn committed_kind() -> TypeId {
    TypeId::of::<TransitionCommitted>()
}

fn aborted_kind() -> TypeId {
    TypeId::of::<TransitionAborted>()
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

const OUTCOME_TY: TypeDescriptor = TypeDescriptor::of::<OutcomeLog>(OutcomeLog::NAME);

static OUTCOME_HOOKS: [HookDescriptor; 2] = [
    HookDescriptor::new(
        1,
        OUTCOME_TY,
        TransitionCommitted::NAME,
        committed_kind,
        no_dependencies,
        committed_call,
    ),
    HookDescriptor::new(
        2,
        OUTCOME_TY,
        TransitionAborted::NAME,
        aborted_kind,
        no_dependencies,
        aborted_call,
    ),
];

fn outcome_hooks() -> &'static [HookDescriptor] {
    &OUTCOME_HOOKS
}

/// An operator-owned singleton no auth switch affects, observing every outcome.
pub(super) struct OutcomeLog;

impl Component for OutcomeLog {
    type Handle = Arc<Self>;

    const ID: &'static str = "outcome-log";
    const NAME: &'static str = "OutcomeLog";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

type OutcomeFactoryFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>>;

fn construct_outcome_log(_context: &mut ComponentConstructionContext) -> OutcomeFactoryFuture<'_> {
    Box::pin(async move {
        Ok(BoxedComponent {
            ty: OUTCOME_TY,
            value: Box::new(Injectable::into_stored(Arc::new(OutcomeLog))),
        })
    })
}

fn outcome_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_outcome_log,
        dependencies: no_dependencies,
        default: true,
    }]
}

/// The outcome log's descriptor, retained across generations through its typed snapshot
/// adapter.
pub(super) static OUTCOME_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: OutcomeLog::ID,
    name: OutcomeLog::NAME,
    ty: OUTCOME_TY,
    scope: &Singleton,
    condition: None,
    factories: outcome_factories,
    hooks: outcome_hooks,
    generation_snapshot: ComponentDescriptor::typed_snapshot::<OutcomeLog>(),
};
