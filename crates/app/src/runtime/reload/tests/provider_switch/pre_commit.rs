//! The #287 acceptance path: transition-proposal and transition-decision hooks inspect
//! the Authenticator provider switch and may reject it before anything is published.

use tempfile::TempDir;
use upwell_di::{Component as _, NodeAction, ReasonKind};
use upwell_hooks::HookRole;

use super::fixture::{
    FallbackAuthenticator, PrimaryAuthenticator, build_auth_app_with, clear_observed_modes,
    config_dir_of, lock_test_guard, observed_modes, write_auth_config,
};
use super::gate::AuthGate;
use super::gate_hooks::{
    AUDIT_COMPONENT, AuditLog, Behavior, Controls, HOOKED_GATE_COMPONENT, SECRET_PANIC, seen,
    seen_hooks, set_controls,
};
use crate::{
    App, RuntimeTransitionReport, TransitionFailure, TransitionHookOutcome, TransitionPhase,
};

async fn hooked_app(controls: Controls) -> (TempDir, App<()>) {
    let built = build_auth_app_with("primary", &HOOKED_GATE_COMPONENT, &[&AUDIT_COMPONENT]).await;

    clear_observed_modes().await;
    set_controls(controls);

    built
}

fn hook_records(
    report: &RuntimeTransitionReport,
) -> Vec<(
    TransitionPhase,
    &'static str,
    HookRole,
    TransitionHookOutcome,
)> {
    report
        .transition_hooks()
        .iter()
        .map(|hook| (hook.phase, hook.component, hook.role, hook.outcome))
        .collect()
}

/// Asserts that the attempt left the primary generation active and unchanged.
fn assert_primary_still_active(app: &App<()>, generation: u64, config_generation: u64) {
    let runtime = app.runtime();
    let root = runtime.root();

    assert_eq!(runtime.generation().get(), generation);
    assert_eq!(runtime.config_generation(), config_generation);
    assert!(root.get::<PrimaryAuthenticator>().is_some());
    assert!(root.get::<FallbackAuthenticator>().is_none());
}

#[tokio::test]
async fn proposal_hooks_see_why_the_authenticator_is_replaced_before_construction() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hooked_app(Controls::default()).await;

    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("every hook accepts the switch");
    let seen = seen();
    let proposal = seen
        .iter()
        .find(|seen| seen.hook == "gate-proposal")
        .expect("the affected gate's proposal hook ran");
    let decision = seen
        .iter()
        .find(|seen| seen.hook == "gate-decision")
        .expect("the affected gate's decision hook ran");

    assert!(report.is_committed());
    assert_eq!(
        seen_hooks(),
        [
            "gate-proposal",
            "policy",
            "proposal-observer",
            "gate-decision",
            "decision-observer",
        ],
        "participants run in registration order, observers after them, and the unaffected \
         audit log's targeted participant never runs"
    );
    assert_eq!(proposal.phase, TransitionPhase::Proposal);
    assert_eq!(
        proposal.components,
        [
            (FallbackAuthenticator::ID, NodeAction::Add),
            (AuthGate::ID, NodeAction::Replace),
            (PrimaryAuthenticator::ID, NodeAction::Remove),
        ],
        "the hook sees every affected consumer and provider"
    );
    assert_eq!(proposal.primary_reasons, [ReasonKind::Removed]);
    assert_eq!(proposal.changed, ["auth"]);
    assert_eq!(
        proposal.staged_mode, "fallback",
        "CfgNext reads the staged value"
    );
    assert_eq!(
        proposal.constructions, 0,
        "proposal runs before construction"
    );
    assert_eq!(proposal.config_hooks, 0, "no config-reload hook ran yet");
    assert_eq!(decision.phase, TransitionPhase::Decision);
    assert_eq!(
        decision.constructions, 1,
        "decision runs after the candidate provider was constructed"
    );
    assert_eq!(
        hook_records(&report),
        [
            (
                TransitionPhase::Proposal,
                AuthGate::NAME,
                HookRole::Participant,
                TransitionHookOutcome::Completed
            ),
            (
                TransitionPhase::Proposal,
                AuditLog::NAME,
                HookRole::Participant,
                TransitionHookOutcome::Completed
            ),
            (
                TransitionPhase::Proposal,
                AuditLog::NAME,
                HookRole::Observer,
                TransitionHookOutcome::Completed
            ),
            (
                TransitionPhase::Decision,
                AuthGate::NAME,
                HookRole::Participant,
                TransitionHookOutcome::Completed
            ),
            (
                TransitionPhase::Decision,
                AuditLog::NAME,
                HookRole::Observer,
                TransitionHookOutcome::Completed
            ),
        ]
    );
    assert_eq!(
        report
            .phases()
            .iter()
            .map(|timing| timing.phase)
            .collect::<Vec<_>>(),
        [
            TransitionPhase::Stage,
            TransitionPhase::Evaluate,
            TransitionPhase::Plan,
            TransitionPhase::Proposal,
            TransitionPhase::Construct,
            TransitionPhase::Hooks,
            TransitionPhase::Decision,
            TransitionPhase::Commit,
        ]
    );
}

#[tokio::test]
async fn proposal_rejection_constructs_nothing_and_changes_nothing() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hooked_app(Controls {
        policy: Behavior::Reject("auth-freeze"),
        ..Controls::default()
    })
    .await;
    let generation = app.runtime().generation().get();
    let config_generation = app.runtime().config_generation();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("the operator policy rejects the switch");
    let (report, error) = super::super::rejected(error);

    super::super::assert_rejected(
        &report,
        TransitionPhase::Proposal,
        TransitionFailure::HookRejected {
            component: AuditLog::NAME,
        },
    );
    assert!(matches!(
        error,
        crate::Error::TransitionHookRejected {
            kind: "transition_proposal",
            component: "AuditLog",
            reason: "auth-freeze",
        }
    ));
    assert_eq!(
        report.transition_hooks().last().map(|hook| hook.outcome),
        Some(TransitionHookOutcome::Rejected {
            reason: "auth-freeze"
        })
    );
    assert_eq!(
        seen_hooks(),
        ["gate-proposal", "policy"],
        "no observer and no decision hook runs for a rejected proposal"
    );
    assert!(
        observed_modes().await.is_empty(),
        "a proposal rejection constructs no candidate"
    );
    assert_primary_still_active(&app, generation, config_generation);

    set_controls(Controls::default());

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("a later reload commits once the policy accepts");

    assert!(report.is_committed());
}

#[tokio::test]
async fn decision_rejection_drops_the_constructed_candidate() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hooked_app(Controls {
        gate_decision: Behavior::Reject("gate-not-ready"),
        ..Controls::default()
    })
    .await;
    let generation = app.runtime().generation().get();
    let config_generation = app.runtime().config_generation();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("the gate rejects the constructed candidate");
    let (report, _error) = super::super::rejected(error);

    super::super::assert_rejected(
        &report,
        TransitionPhase::Decision,
        TransitionFailure::HookRejected {
            component: AuthGate::NAME,
        },
    );
    assert_eq!(
        observed_modes().await,
        ["fallback"],
        "the candidate was constructed before the decision"
    );
    assert_eq!(
        seen_hooks(),
        [
            "gate-proposal",
            "policy",
            "proposal-observer",
            "gate-decision"
        ],
        "no decision observer runs for a rejected decision"
    );
    assert_primary_still_active(&app, generation, config_generation);
}

#[tokio::test]
async fn hook_panics_are_redacted_and_later_reloads_still_work() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hooked_app(Controls {
        gate_proposal: Behavior::Panic,
        ..Controls::default()
    })
    .await;
    let generation = app.runtime().generation().get();
    let config_generation = app.runtime().config_generation();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("a panicking participant rejects the switch");
    let rendered = format!("{error} {error:?}");
    let (report, error) = super::super::rejected(error);

    super::super::assert_rejected(
        &report,
        TransitionPhase::Proposal,
        TransitionFailure::HookPanicked {
            component: AuthGate::NAME,
        },
    );
    assert!(matches!(
        error,
        crate::Error::TransitionHookFailed {
            kind: "transition_proposal",
            ..
        }
    ));
    assert!(!rendered.contains(SECRET_PANIC), "rejection rendering");
    assert!(!error.to_string().contains(SECRET_PANIC), "error rendering");
    assert_primary_still_active(&app, generation, config_generation);

    set_controls(Controls {
        observer_panics: true,
        ..Controls::default()
    });

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("a panicking observer never rejects");

    assert!(report.is_committed());
    assert_eq!(
        report
            .transition_hooks()
            .last()
            .map(|hook| (hook.role, hook.outcome)),
        Some((HookRole::Observer, TransitionHookOutcome::Panicked))
    );
    assert!(!format!("{report:?}").contains(SECRET_PANIC));
}

#[tokio::test]
async fn unchanged_sources_fire_no_transition_hooks() {
    let _guard = lock_test_guard().await;
    let (_dir, app) = hooked_app(Controls::default()).await;

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("an unchanged source is a no-op");

    assert!(!report.is_committed());
    assert!(seen_hooks().is_empty());
    assert!(report.transition_hooks().is_empty());
}

#[tokio::test]
async fn a_proposal_hook_cannot_reload_its_own_runtime() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hooked_app(Controls::default()).await;

    set_controls(Controls {
        reload_from_proposal: Some(app.runtime().clone()),
        ..Controls::default()
    });
    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the running transition is unaffected by the nested request");
    let proposal = seen()
        .into_iter()
        .find(|seen| seen.hook == "gate-proposal")
        .expect("the gate's proposal hook ran");

    assert!(report.is_committed());
    assert_eq!(
        proposal.nested_reload_rejected,
        Some(true),
        "the nested reload fails fast with ReentrantReload"
    );
}
