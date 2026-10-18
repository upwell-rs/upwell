//! Panicking hook calls are contained and leave the manager usable.

use super::*;

#[test]
fn hook_panics_are_isolated_and_the_manager_remains_usable() {
    let (manager, _resolver) = attached(vec![panicking_hook()]);

    for _ in 0..2 {
        let fired = futures::executor::block_on(manager.fire_all::<Startup>(&()));

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

    let fired = futures::executor::block_on(manager.fire_all::<Startup>(&()));

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
