use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use upwell_core::{DependencyDescriptor, ResolverCtx, ResolverSet, TypeDescriptor};

use super::{
    Error, HookCall, HookDescriptor, HookKind, HookManager, HookParticipation, HookRole,
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
static TEST_GUARD: Mutex<()> = Mutex::new(());

fn record(call: &'static str) {
    CALLS.lock().expect("call log").push(call);
}

fn take_calls() -> Vec<&'static str> {
    std::mem::take(&mut *CALLS.lock().expect("call log"))
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn startup_type_id() -> TypeId {
    TypeId::of::<Startup>()
}

fn gate_type_id() -> TypeId {
    TypeId::of::<Gate>()
}

fn notice_type_id() -> TypeId {
    TypeId::of::<Notice>()
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

fn hook<T: 'static>(name: &'static str, kind_ty: fn() -> TypeId, call: HookCall) -> HookDescriptor {
    HookDescriptor::new(
        0,
        TypeDescriptor::of::<T>(name),
        "test",
        kind_ty,
        no_dependencies,
        call,
    )
}

fn panicking_hook() -> HookDescriptor {
    hook::<PanickingComponent>(
        "PanickingComponent",
        startup_type_id,
        panicking_call as HookCall,
    )
}

fn synchronously_panicking_hook() -> HookDescriptor {
    hook::<SynchronouslyPanickingComponent>(
        "SynchronouslyPanickingComponent",
        startup_type_id,
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

#[test]
fn hook_panics_are_isolated_and_the_manager_remains_usable() {
    let (manager, _resolver) = attached(vec![panicking_hook()]);

    for _ in 0..2 {
        let fired = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));

        assert!(fired.ended, "a panicking participant is fail-closed");
        assert!(matches!(
            fired.participants.as_slice(),
            [(
                _,
                Err(Error::Panicked {
                    hook: "startup",
                    component: "PanickingComponent"
                })
            )]
        ));
        assert!(
            !fired.participants[0]
                .1
                .as_ref()
                .expect_err("the panic is an error")
                .to_string()
                .contains("sensitive")
        );
    }
}

#[test]
fn synchronous_hook_call_panics_are_isolated() {
    let (manager, _resolver) = attached(vec![synchronously_panicking_hook()]);

    let fired = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));

    assert!(matches!(
        fired.participants.as_slice(),
        [(
            _,
            Err(Error::Panicked {
                hook: "startup",
                component: "SynchronouslyPanickingComponent"
            })
        )]
    ));
    assert!(
        !fired.participants[0]
            .1
            .as_ref()
            .expect_err("the panic is an error")
            .to_string()
            .contains("sensitive")
    );
}

#[test]
fn expired_resolver_context_ends_the_event_at_the_first_participant() {
    let (manager, resolver) = attached(vec![panicking_hook(), synchronously_panicking_hook()]);

    drop(resolver);

    let fired = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));

    assert!(fired.ended);
    assert!(matches!(
        fired.participants.as_slice(),
        [(_, Err(Error::ResolverUnavailable))]
    ));
}

#[test]
fn attaching_a_new_generation_replaces_an_expired_context() {
    let (manager, initial) = attached(vec![panicking_hook()]);

    drop(initial);

    let replacement: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());

    manager.attach(Arc::downgrade(&replacement));

    let fired = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));

    assert!(matches!(
        fired.participants.as_slice(),
        [(
            _,
            Err(Error::Panicked {
                hook: "startup",
                ..
            })
        )]
    ));
}

#[test]
fn resolver_provider_is_loaded_once_per_fire() {
    let manager = HookManager::new(vec![panicking_hook()]);
    let loads = Arc::new(AtomicUsize::new(0));
    let provider_loads = Arc::clone(&loads);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());

    manager.attach_resolver_provider(move || {
        provider_loads.fetch_add(1, Ordering::Relaxed);

        Some(Arc::clone(&resolver))
    });

    let _ = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));
    let _ = futures::executor::block_on(manager.fire::<Startup>(&(), |_| true));

    assert_eq!(loads.load(Ordering::Relaxed), 2);
}

#[test]
fn observers_run_after_every_participant_accepted() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<First>("First", gate_type_id, observe_call).with_role(HookRole::Observer),
        hook::<Second>("Second", gate_type_id, accept_call),
        hook::<Third>("Third", gate_type_id, accept_call),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire::<Gate>(&(), |_| true));

    assert!(!fired.ended);
    assert_eq!(
        take_calls(),
        ["accept", "accept", "observe"],
        "observers run only once the participants settled, whatever their registration order"
    );
    assert_eq!(names(&fired.participants), ["Second", "Third"]);
    assert_eq!(names(&fired.observers), ["First"]);
}

#[test]
fn an_ending_participant_skips_later_participants_and_every_observer() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<First>("First", gate_type_id, deny_call),
        hook::<Second>("Second", gate_type_id, accept_call),
        hook::<Third>("Third", gate_type_id, observe_call).with_role(HookRole::Observer),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire::<Gate>(&(), |_| true));

    assert!(fired.ended, "the kind ends the event on a `false` verdict");
    assert_eq!(take_calls(), ["deny"]);
    assert_eq!(names(&fired.participants), ["First"]);
    assert!(fired.observers.is_empty());
}

#[test]
fn observer_failures_are_reported_without_ending_the_event() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<First>("First", gate_type_id, failing_observe_call).with_role(HookRole::Observer),
        hook::<Second>("Second", gate_type_id, observe_call).with_role(HookRole::Observer),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire::<Gate>(&(), |_| true));

    assert!(!fired.ended);
    assert!(matches!(
        fired.observers.as_slice(),
        [(_, Err(Error::MissingParam("audit sink"))), (_, Ok(()))]
    ));
}

#[test]
fn observer_only_kinds_never_run_participants() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<First>("First", notice_type_id, failing_observe_call),
        hook::<Second>("Second", notice_type_id, observe_call),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire::<Notice>(&(), |_| true));

    assert!(
        fired.participants.is_empty(),
        "a descriptor declaring a participant is still an observer of an observer-only kind"
    );
    assert!(!fired.ended);
    assert_eq!(names(&fired.observers), ["First", "Second"]);
    assert!(!manager.component_participates::<Notice>(TypeId::of::<First>()));
    assert_eq!(Shutdown::PARTICIPATION, HookParticipation::ObserversOnly);
}

#[test]
fn always_routed_hooks_bypass_the_firing_sites_selection() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<First>("First", gate_type_id, accept_call),
        hook::<Second>("Second", gate_type_id, accept_call).with_routing(HookRouting::Always),
        hook::<Third>("Third", gate_type_id, observe_call)
            .with_role(HookRole::Observer)
            .with_routing(HookRouting::Always),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire::<Gate>(&(), |_| false));

    assert_eq!(names(&fired.participants), ["Second"]);
    assert_eq!(names(&fired.observers), ["Third"]);
    assert!(manager.component_participates::<Gate>(TypeId::of::<First>()));
    assert!(!manager.component_participates::<Gate>(TypeId::of::<Third>()));
}
