//! Streamed firing: completion-order items and the ending participant as the last item.

use super::*;

#[test]
fn streamed_observers_arrive_in_completion_order_and_collect_in_registration_order() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let hooks = || {
        vec![
            hook::<Gate, First>("First", accept_call),
            hook::<Gate, Second>("Second", waiting_observe_call).with_role(HookRole::Observer),
            hook::<Gate, Third>("Third", releasing_observe_call).with_role(HookRole::Observer),
        ]
    };
    let (manager, _resolver) = attached(hooks());

    take_calls();
    arm_release();

    let streamed = futures::executor::block_on(
        manager
            .fire_all_streamed::<Gate>(&())
            .map(|fired| match fired {
                FiredHook::Participant { component, .. } => ("participant", component.name),
                FiredHook::Observer { component, .. } => ("observer", component.name),
            })
            .collect::<Vec<_>>(),
    );

    assert_eq!(
        streamed,
        [
            ("participant", "First"),
            ("observer", "Third"),
            ("observer", "Second"),
        ],
        "observers run concurrently and stream as they complete"
    );
    assert_eq!(take_calls(), ["accept", "released", "waited"]);

    arm_release();

    let collected = futures::executor::block_on(manager.fire_all::<Gate>(&()));

    assert_eq!(
        names(&collected.observers),
        ["Second", "Third"],
        "collected observers keep registration order"
    );
}

#[test]
fn a_streamed_ending_participant_is_the_last_item() {
    let _guard = TEST_GUARD.lock().expect("test guard");
    let (manager, _resolver) = attached(vec![
        hook::<Gate, First>("First", deny_call),
        hook::<Gate, Second>("Second", observe_call).with_role(HookRole::Observer),
    ]);

    let streamed = futures::executor::block_on(
        manager
            .fire_streamed::<Gate>(&(), |_| true)
            .collect::<Vec<_>>(),
    );

    assert_eq!(streamed.len(), 1);
    assert!(streamed[0].ends_event());
    assert_eq!(streamed[0].component().name, "First");
}
