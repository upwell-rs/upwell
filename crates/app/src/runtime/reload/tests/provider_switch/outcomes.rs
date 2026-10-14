//! The #288 acceptance path: transition outcome observers receive the final report of
//! every committed or aborted Authenticator switch exactly once, after the attempt
//! released its leases, whatever requested the reload.

use tempfile::TempDir;
use upwell_config::{ReloadCause, ReloadTarget};
use upwell_di::{Component as _, NodeAction};

use super::fixture::{
    FallbackAuthenticator, PrimaryAuthenticator, build_auth_app_with, config_dir_of,
    fail_fallback_factory, lock_test_guard, write_auth_config,
};
use super::gate::AuthGate;
use super::gate_hooks::{AUDIT_COMPONENT, Behavior, Controls, HOOKED_GATE_COMPONENT, set_controls};
use super::outcome_hooks::{
    OUTCOME_COMPONENT, OutcomeControls, SECRET_OBSERVER_PANIC, observed, observed_kinds,
    set_outcome_controls,
};
use crate::test_support::capture_events;
use crate::{App, TransitionFailure, TransitionOutcome, TransitionPhase};

async fn observed_app(gate: Controls, outcome: OutcomeControls) -> (TempDir, App<()>) {
    let built = build_auth_app_with(
        "primary",
        &HOOKED_GATE_COMPONENT,
        &[&AUDIT_COMPONENT, &OUTCOME_COMPONENT],
    )
    .await;

    fail_fallback_factory(false);
    set_controls(gate);
    set_outcome_controls(outcome);

    built
}

#[tokio::test]
async fn committed_observers_receive_the_final_report_of_the_authenticator_switch() {
    let _guard = lock_test_guard().await;
    let (dir, app) = observed_app(Controls::default(), OutcomeControls::Record).await;

    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the switch commits");
    let observed = observed();

    assert_eq!(observed_kinds(), ["transition_committed"], "exactly once");

    let seen = &observed[0].report;

    assert_eq!(seen.attempt(), report.attempt());
    assert_eq!(seen.cause(), ReloadCause::Requested);
    assert_eq!(
        seen.outcome(),
        &TransitionOutcome::Committed {
            generation: app.runtime().generation(),
        },
        "the observer receives the committed generation"
    );
    assert_eq!(
        seen.components()
            .iter()
            .map(|component| (component.component, component.action))
            .collect::<Vec<_>>(),
        [
            (FallbackAuthenticator::ID, NodeAction::Add),
            (AuthGate::ID, NodeAction::Replace),
            (PrimaryAuthenticator::ID, NodeAction::Remove),
        ],
        "the observer receives the final action report"
    );
    assert_eq!(seen.phases().len(), report.phases().len());
}

#[tokio::test]
async fn every_aborted_attempt_reaches_aborted_observers_exactly_once() {
    let _guard = lock_test_guard().await;
    let (dir, app) = observed_app(
        Controls {
            policy: Behavior::Reject("auth-freeze"),
            ..Controls::default()
        },
        OutcomeControls::Record,
    )
    .await;
    let generation = app.runtime().generation();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let rejected = app.runtime().reload_config().await;

    set_controls(Controls {
        gate_proposal: Behavior::Panic,
        ..Controls::default()
    });

    let panicked = app.runtime().reload_config().await;

    set_controls(Controls::default());
    fail_fallback_factory(true);

    let failed = app.runtime().reload_config().await;

    fail_fallback_factory(false);

    let aborted = observed();

    assert!(rejected.is_err() && panicked.is_err() && failed.is_err());
    assert_eq!(
        observed_kinds(),
        [
            "transition_aborted",
            "transition_aborted",
            "transition_aborted"
        ],
        "each aborted attempt reaches the observers exactly once"
    );
    assert_eq!(
        aborted
            .iter()
            .map(|observed| observed.report.outcome().clone())
            .collect::<Vec<_>>(),
        [
            TransitionOutcome::Rejected {
                phase: TransitionPhase::Proposal,
                failure: TransitionFailure::HookRejected {
                    component: "AuditLog",
                },
            },
            TransitionOutcome::Rejected {
                phase: TransitionPhase::Proposal,
                failure: TransitionFailure::HookPanicked {
                    component: AuthGate::NAME,
                },
            },
            TransitionOutcome::Rejected {
                phase: TransitionPhase::Construct,
                failure: TransitionFailure::ConstructionFailed,
            },
        ]
    );

    for observed in &aborted {
        let rendered = format!("{:?}", observed.report);

        assert_eq!(observed.report.runtime_generation(), generation);
        assert!(
            !rendered.contains("mode") && !rendered.contains("unavailable"),
            "the aborted context carries no config values or error messages"
        );
    }

    assert_eq!(
        app.runtime().generation(),
        generation,
        "nothing was published"
    );
}

#[tokio::test]
async fn failing_and_panicking_observers_never_roll_back_or_block_later_reloads() {
    let _guard = lock_test_guard().await;
    let (dir, app) = observed_app(Controls::default(), OutcomeControls::Panic).await;
    let generation = app.runtime().generation().get();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let (result, events) = capture_events(app.runtime().reload_config()).await;
    let report = result.expect("a panicking observer cannot reject a committed attempt");

    assert!(report.is_committed());
    assert_eq!(app.runtime().generation().get(), generation + 1);
    assert!(
        app.runtime()
            .root()
            .get::<FallbackAuthenticator>()
            .is_some(),
        "the published generation stays published"
    );
    assert!(
        events.iter().any(|event| {
            event.field("hook") == Some("transition_committed")
                && event.field("outcome") == Some("panicked")
        }),
        "the observer panic is traced"
    );
    assert!(
        events
            .iter()
            .all(|event| !format!("{event:?}").contains(SECRET_OBSERVER_PANIC)),
        "the panic payload is never traced"
    );

    set_outcome_controls(OutcomeControls::Fail);
    write_auth_config(&config_dir_of(&dir), "primary");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("a failing observer cannot reject a later reload");

    assert!(report.is_committed());
    assert_eq!(app.runtime().generation().get(), generation + 2);
    assert_eq!(observed_kinds(), ["transition_committed"]);
}

#[tokio::test]
async fn an_observer_may_request_a_follow_up_reload() {
    let _guard = lock_test_guard().await;
    let (dir, app) = observed_app(Controls::default(), OutcomeControls::Record).await;

    set_outcome_controls(OutcomeControls::FollowUp(app.runtime().clone()));
    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the switch commits");
    let observed = observed();

    assert!(report.is_committed());
    assert_eq!(
        observed[0].follow_up_reentrant,
        Some(false),
        "observers run after the leases are released, so a follow-up reload runs"
    );
}

#[tokio::test]
async fn triggered_reloads_reach_the_same_observers_with_their_cause() {
    let _guard = lock_test_guard().await;
    let (dir, app) = observed_app(Controls::default(), OutcomeControls::Record).await;

    write_auth_config(&config_dir_of(&dir), "fallback");
    app.runtime()
        .trigger_reload(ReloadCause::Signal)
        .await
        .expect("the SIGHUP-triggered switch commits");
    write_auth_config(&config_dir_of(&dir), "primary");
    app.runtime()
        .trigger_reload(ReloadCause::FileWatch)
        .await
        .expect("the file-watch-triggered switch commits");

    assert_eq!(
        observed()
            .iter()
            .map(|observed| (observed.kind, observed.report.cause()))
            .collect::<Vec<_>>(),
        [
            ("transition_committed", ReloadCause::Signal),
            ("transition_committed", ReloadCause::FileWatch),
        ]
    );
}
