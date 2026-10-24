//! Participant and observer ordering, event ending, routing and targeting.

use super::*;

#[test]
fn observers_run_after_every_participant_accepted() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<Gate, First>("First", observe_call).with_role(HookRole::Observer),
        hook::<Gate, Second>("Second", accept_call),
        hook::<Gate, Third>("Third", accept_call),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire_all::<Gate>(&()));

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
        hook::<Gate, First>("First", deny_call),
        hook::<Gate, Second>("Second", accept_call),
        hook::<Gate, Third>("Third", observe_call).with_role(HookRole::Observer),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire_all::<Gate>(&()));

    assert!(fired.ended, "the kind ends the event on a `false` verdict");
    assert_eq!(take_calls(), ["deny"]);
    assert_eq!(names(&fired.participants), ["First"]);
    assert!(fired.observers.is_empty());
}

#[test]
fn observer_failures_are_reported_without_ending_the_event() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<Gate, First>("First", failing_observe_call).with_role(HookRole::Observer),
        hook::<Gate, Second>("Second", observe_call).with_role(HookRole::Observer),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire_all::<Gate>(&()));

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
        hook::<Notice, First>("First", failing_observe_call),
        hook::<Notice, Second>("Second", observe_call),
    ]);

    take_calls();

    let fired = futures::executor::block_on(manager.fire_all::<Notice>(&()));

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
        hook::<Gate, First>("First", accept_call),
        hook::<Gate, Second>("Second", accept_call).with_routing(HookRouting::Always),
        hook::<Gate, Third>("Third", observe_call)
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

#[test]
fn fire_all_ignores_targeting() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![hook::<Gate, First>("First", accept_call)]);

    let selected = futures::executor::block_on(manager.fire::<Gate>(&(), |_| false));
    let all = futures::executor::block_on(manager.fire_all::<Gate>(&()));

    assert!(selected.participants.is_empty());
    assert_eq!(names(&all.participants), ["First"]);
}
