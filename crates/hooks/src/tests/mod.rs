use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use futures::channel::oneshot;
use upwell_core::{ResolverCtx, ResolverSet, TypeDescriptor};

use super::{
    Error, FiredHook, HookCall, HookDescriptor, HookKind, HookManager, HookParticipation, HookRole,
    HookRouting, Shutdown, Startup,
};

type CallFuture<'a> = Pin<Box<dyn Future<Output = super::Result<Box<dyn Any + Send>>> + Send + 'a>>;

struct PanickingComponent;
struct SynchronouslyPanickingComponent;
struct First;
struct Second;
struct Third;

/// A participant-and-observer kind whose `bool` output ends the event when `false`.
struct Gate;

impl HookKind for Gate {
    type Output = bool;
    type Cx = ();

    const NAME: &'static str = "gate";

    fn ends_event(outcome: &super::Result<bool>) -> bool {
        !matches!(outcome, Ok(true))
    }
}

/// An observer-only kind.
struct Notice;

impl HookKind for Notice {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "notice";
    const PARTICIPATION: HookParticipation = HookParticipation::ObserversOnly;
}

static CALLS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static RELEASE: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);
static RELEASED: Mutex<Option<oneshot::Receiver<()>>> = Mutex::new(None);
static TEST_GUARD: Mutex<()> = Mutex::new(());

fn record(call: &'static str) {
    CALLS.lock().expect("call log").push(call);
}

fn take_calls() -> Vec<&'static str> {
    std::mem::take(&mut *CALLS.lock().expect("call log"))
}

fn panicking_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    Box::pin(async move { panic!("sensitive panic payload") })
}

fn synchronously_panicking_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    panic!("sensitive synchronous panic payload")
}

fn accept_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    Box::pin(async move {
        record("accept");

        Ok(Box::new(true) as Box<dyn Any + Send>)
    })
}

fn deny_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    Box::pin(async move {
        record("deny");

        Ok(Box::new(false) as Box<dyn Any + Send>)
    })
}

fn observe_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    Box::pin(async move {
        record("observe");

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn failing_observe_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    Box::pin(async move {
        record("failing-observe");

        Err(Error::MissingParam("audit sink"))
    })
}

/// Waits until [`releasing_observe_call`] runs, so it completes after a later observer.
fn waiting_observe_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    let released = RELEASED.lock().expect("release receiver").take();

    Box::pin(async move {
        released
            .expect("a release channel is armed")
            .await
            .expect("the releasing observer runs");
        record("waited");

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn releasing_observe_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> CallFuture<'a> {
    let release = RELEASE.lock().expect("release sender").take();

    Box::pin(async move {
        record("released");
        release
            .expect("a release channel is armed")
            .send(())
            .expect("the waiting observer is pending");

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn arm_release() {
    let (sender, receiver) = oneshot::channel();

    *RELEASE.lock().expect("release sender") = Some(sender);
    *RELEASED.lock().expect("release receiver") = Some(receiver);
}

/// A targeted participant hook of kind `K` on the component type `T` named `name`.
fn hook<K: HookKind, T: 'static>(name: &'static str, call: HookCall) -> HookDescriptor {
    HookDescriptor::of::<K>(0, TypeDescriptor::of::<T>(name), call)
}

fn panicking_hook() -> HookDescriptor {
    hook::<Startup, PanickingComponent>("PanickingComponent", panicking_call as HookCall)
}

fn synchronously_panicking_hook() -> HookDescriptor {
    hook::<Startup, SynchronouslyPanickingComponent>(
        "SynchronouslyPanickingComponent",
        synchronously_panicking_call as HookCall,
    )
}

fn attached(hooks: Vec<HookDescriptor>) -> (HookManager, Arc<dyn ResolverCtx + Send + Sync>) {
    let manager = HookManager::new(hooks);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());

    manager.attach(Arc::downgrade(&resolver));

    (manager, resolver)
}

fn names<T>(outcomes: &[(TypeDescriptor, T)]) -> Vec<&'static str> {
    outcomes
        .iter()
        .map(|(component, _)| component.name)
        .collect()
}

mod context;
mod dispatch;
mod isolation;
mod streaming;
