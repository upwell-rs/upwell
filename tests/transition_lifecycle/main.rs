//! End-to-end acceptance of the runtime transition hook and report model (#211) through the
//! `upwell` facade: switching the effective `Authenticator` provider in both directions
//! runs every phase in order — proposal inspection, the `ConfigReload` hooks once,
//! decision rejection with rollback, the aborted observer, then a committed attempt whose
//! displaced instances retire before the committed observer receives the final report.
#![allow(dead_code)]

mod app;

use std::sync::Arc;

use app::{
    AuthGate, AuthWatcher, ChangeFreeze, CustomAuthenticator, DefaultAuthenticator, Event,
    TransitionAudit, build, events, freeze, lock, retirement_of, write_mode,
};
use upwell::{
    AppError, Component, NodeAction, RetirementReason, TransitionFailure, TransitionOutcome,
    TransitionPhase,
};

fn kinds(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            Event::Proposal { .. } => "proposal",
            Event::ConfigReload { .. } => "config_reload",
            Event::Decision { .. } => "decision",
            Event::Aborted(_) => "aborted",
            Event::Retired { .. } => "retired",
            Event::Committed(_) => "committed",
            Event::Shutdown { .. } => "shutdown",
        })
        .collect()
}

/// The provider a switch enables and the one it displaces, with their schemes.
struct Switch {
    from: &'static str,
    to: &'static str,
    added: &'static str,
    removed: &'static str,
}

/// Drives one provider switch: a frozen attempt that rejects at decision and rolls back,
/// then an unfrozen attempt that commits.
async fn switch_provider(switch: Switch) {
    let (_dir, config_dir, app) = build(switch.from).await;
    let runtime = app.runtime();
    let base = runtime.generation();
    let old_gate = runtime.root().get::<AuthGate>().expect("the gate resolves");
    let expected_plan = [
        (switch.added, NodeAction::Add),
        (AuthGate::ID, NodeAction::Replace),
        (AuthWatcher::ID, NodeAction::Replace),
        (switch.removed, NodeAction::Remove),
    ];

    assert_eq!(old_gate.scheme(), switch.from);

    freeze(Some("change-freeze"));
    write_mode(&config_dir, switch.to);

    let rejected = runtime
        .reload_config()
        .await
        .expect_err("the change freeze rejects the switch");
    let aborted = events();

    assert_eq!(
        kinds(&aborted),
        ["proposal", "config_reload", "decision", "aborted"],
        "a rejected attempt runs every pre-commit phase, then its aborted observer"
    );
    assert_inspected(&aborted, &expected_plan, switch.from, switch.to);

    let AppError::TransitionRejected(rejection) = rejected else {
        panic!("a decision rejection is a transition rejection: {rejected:?}");
    };
    let Event::Aborted(observed) = &aborted[3] else {
        unreachable!("the kinds were asserted");
    };

    assert_eq!(observed.attempt(), rejection.report().attempt());
    assert!(matches!(
        observed.outcome(),
        TransitionOutcome::Rejected {
            phase: TransitionPhase::Decision,
            failure: TransitionFailure::HookRejected { .. },
        }
    ));
    assert!(observed.displaced().is_empty(), "nothing was displaced");
    assert_eq!(
        runtime.generation(),
        base,
        "the rejection published nothing"
    );
    assert!(
        Arc::ptr_eq(&runtime.root().get::<AuthGate>().expect("gate"), &old_gate),
        "the rejection rolled back to the active gate"
    );

    freeze(None);

    let report = runtime
        .reload_config()
        .await
        .expect("the unfrozen switch commits");
    let committed = events();
    let new_gate = runtime.root().get::<AuthGate>().expect("the new gate");

    assert_eq!(
        kinds(&committed),
        [
            "proposal",
            "config_reload",
            "decision",
            "retired",
            "retired",
            "committed"
        ],
        "a committed attempt retires the displaced instances, then observes the commit"
    );
    assert_inspected(&committed, &expected_plan, switch.from, switch.to);
    assert_eq!(new_gate.scheme(), switch.to);
    assert_eq!(
        retirement_of(&committed, AuthGate::ID),
        [(RetirementReason::Replaced, switch.from)],
        "the displaced gate retires against its own instance"
    );
    assert_eq!(
        retirement_of(&committed, switch.removed),
        [(RetirementReason::Removed, switch.from)]
    );
    assert!(retirement_of(&committed, ChangeFreeze::ID).is_empty());
    assert!(retirement_of(&committed, TransitionAudit::ID).is_empty());

    let Some(Event::Committed(observed)) = committed.last() else {
        unreachable!("the kinds were asserted");
    };

    assert_eq!(observed.attempt(), report.attempt());
    assert_eq!(
        observed.outcome(),
        &TransitionOutcome::Committed {
            generation: runtime.generation(),
        },
        "the committed observer receives the published generation"
    );
    assert_eq!(observed.displaced(), report.displaced());
    assert!(observed.displaced().contains(&AuthGate::ID));
    assert!(observed.displaced().contains(&switch.removed));
}

/// Asserts what the proposal, config-reload, and decision hooks of one attempt saw.
fn assert_inspected(
    events: &[Event],
    expected_plan: &[(&'static str, NodeAction)],
    from: &str,
    to: &str,
) {
    let mut plan = expected_plan.to_vec();

    plan.sort_unstable();

    for event in events {
        match event {
            Event::Proposal { components, scheme } => {
                let mut components = components.clone();

                components.sort_unstable();

                assert_eq!(
                    components, plan,
                    "the proposal sees every affected component"
                );
                assert_eq!(*scheme, from, "the proposal runs on the active gate");
            }
            Event::ConfigReload { mode } => assert_eq!(mode, to),
            Event::Decision {
                components,
                config_hooks,
            } => {
                let mut components = components.clone();

                components.sort_unstable();

                assert_eq!(components, plan);
                assert_eq!(
                    *config_hooks, 1,
                    "the decision reuses the config-reload outcome instead of re-running it"
                );
            }
            Event::Aborted(_)
            | Event::Retired { .. }
            | Event::Committed(_)
            | Event::Shutdown { .. } => {}
        }
    }
}

#[tokio::test]
async fn enabling_the_custom_authenticator_runs_every_transition_phase() {
    let _guard = lock().await;

    switch_provider(Switch {
        from: "default",
        to: "custom",
        added: CustomAuthenticator::ID,
        removed: DefaultAuthenticator::ID,
    })
    .await;
}

#[tokio::test]
async fn disabling_the_custom_authenticator_runs_every_transition_phase() {
    let _guard = lock().await;

    switch_provider(Switch {
        from: "custom",
        to: "default",
        added: DefaultAuthenticator::ID,
        removed: CustomAuthenticator::ID,
    })
    .await;
}

#[tokio::test]
async fn shutdown_reaches_the_live_instances_after_a_switch() {
    let _guard = lock().await;
    let (_dir, config_dir, app) = build("default").await;
    let runtime = app.runtime().clone();
    let shutdown = app.shutdown_handle();
    let drive = async {
        write_mode(&config_dir, "custom");
        runtime
            .reload_config()
            .await
            .expect("the switch commits while the app runs");
        events();
        shutdown.shutdown();
    };

    let (run, ()) = tokio::join!(app.run(), drive);
    let mut stopped = events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Shutdown { component, scheme } => Some((component, scheme)),
            _ => None,
        })
        .collect::<Vec<_>>();

    stopped.sort_unstable();

    run.expect("the app stops cleanly");
    assert_eq!(
        stopped,
        [
            (AuthGate::ID, "custom"),
            (CustomAuthenticator::ID, "custom"),
        ],
        "shutdown reaches the current gate and the provider the switch added, never the \
         retired default provider"
    );
}
