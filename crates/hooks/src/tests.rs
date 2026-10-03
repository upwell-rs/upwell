use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_core::{DependencyDescriptor, ResolverCtx, ResolverSet, TypeDescriptor};

use super::{Error, HookCall, HookDescriptor, HookKind, HookManager, Startup};

struct PanickingComponent;
struct SynchronouslyPanickingComponent;

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn startup_type_id() -> TypeId {
    TypeId::of::<Startup>()
}

#[allow(clippy::type_complexity)]
fn panicking_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> Pin<Box<dyn Future<Output = super::Result<Box<dyn Any + Send>>> + Send + 'a>> {
    Box::pin(async move { panic!("sensitive panic payload") })
}

#[allow(clippy::type_complexity)]
fn synchronously_panicking_call<'a>(
    _ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> Pin<Box<dyn Future<Output = super::Result<Box<dyn Any + Send>>> + Send + 'a>> {
    panic!("sensitive synchronous panic payload")
}

fn panicking_hook() -> HookDescriptor {
    HookDescriptor::new(
        0,
        TypeDescriptor::of::<PanickingComponent>("PanickingComponent"),
        Startup::NAME,
        startup_type_id,
        no_dependencies,
        panicking_call as HookCall,
    )
}

fn synchronously_panicking_hook() -> HookDescriptor {
    HookDescriptor::new(
        0,
        TypeDescriptor::of::<SynchronouslyPanickingComponent>("SynchronouslyPanickingComponent"),
        Startup::NAME,
        startup_type_id,
        no_dependencies,
        synchronously_panicking_call as HookCall,
    )
}

#[test]
fn hook_panics_are_isolated_and_the_manager_remains_usable() {
    let manager = HookManager::new(vec![panicking_hook()]);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());
    manager.attach(Arc::downgrade(&resolver));

    for _ in 0..2 {
        let outcomes = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));

        assert!(matches!(
            outcomes.as_slice(),
            [(
                _,
                Err(Error::Panicked {
                    hook: "startup",
                    component: "PanickingComponent"
                })
            )]
        ));
        assert!(
            !outcomes[0]
                .1
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("sensitive")
        );
    }
}

#[test]
fn synchronous_hook_call_panics_are_isolated_in_both_runners() {
    let manager = HookManager::new(vec![synchronously_panicking_hook()]);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());
    manager.attach(Arc::downgrade(&resolver));

    let concurrent = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));
    let sequential = futures::executor::block_on(manager.run_until_error::<Startup>(&(), |_| true));

    for outcomes in [concurrent, sequential] {
        assert!(matches!(
            outcomes.as_slice(),
            [(
                _,
                Err(Error::Panicked {
                    hook: "startup",
                    component: "SynchronouslyPanickingComponent"
                })
            )]
        ));
        assert!(
            !outcomes[0]
                .1
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("sensitive")
        );
    }
}

#[test]
fn expired_resolver_context_returns_typed_errors() {
    let manager = HookManager::new(vec![panicking_hook(), synchronously_panicking_hook()]);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());
    manager.attach(Arc::downgrade(&resolver));
    drop(resolver);

    let concurrent = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));
    let sequential = futures::executor::block_on(manager.run_until_error::<Startup>(&(), |_| true));

    assert!(
        concurrent
            .iter()
            .all(|(_, outcome)| matches!(outcome, Err(Error::ResolverUnavailable)))
    );
    assert!(matches!(
        sequential.as_slice(),
        [(_, Err(Error::ResolverUnavailable))]
    ));
}

#[test]
fn attaching_a_new_generation_replaces_an_expired_context() {
    let manager = HookManager::new(vec![panicking_hook()]);
    let initial: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());
    manager.attach(Arc::downgrade(&initial));
    drop(initial);

    let replacement: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());
    manager.attach(Arc::downgrade(&replacement));

    let outcomes = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));

    assert!(matches!(
        outcomes.as_slice(),
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
fn resolver_provider_is_loaded_for_each_hook_run() {
    let manager = HookManager::new(vec![panicking_hook()]);
    let loads = Arc::new(AtomicUsize::new(0));
    let provider_loads = Arc::clone(&loads);
    let resolver: Arc<dyn ResolverCtx + Send + Sync> = Arc::new(ResolverSet::new());

    manager.attach_resolver_provider(move || {
        provider_loads.fetch_add(1, Ordering::Relaxed);

        Some(Arc::clone(&resolver))
    });

    let _ = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));
    let _ = futures::executor::block_on(manager.run::<Startup>(&(), |_| true));

    assert_eq!(loads.load(Ordering::Relaxed), 2);
}
