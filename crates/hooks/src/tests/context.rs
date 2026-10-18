//! The resolver context a manager fires through, across generations.

use super::*;

#[test]
fn expired_resolver_context_ends_the_event_at_the_first_participant() {
    let (manager, resolver) = attached(vec![panicking_hook(), synchronously_panicking_hook()]);

    drop(resolver);

    let fired = futures::executor::block_on(manager.fire_all::<Startup>(&()));

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

    let fired = futures::executor::block_on(manager.fire_all::<Startup>(&()));

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

    let _ = futures::executor::block_on(manager.fire_all::<Startup>(&()));
    let _ = futures::executor::block_on(manager.fire_all::<Startup>(&()));

    assert_eq!(loads.load(Ordering::Relaxed), 2);
}
