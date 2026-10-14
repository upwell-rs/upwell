//! The canonical transition report: every attempt consumes an attempt ID, reports its
//! affected components and outcome, and never renders config values — in the report,
//! the rejection, serialized output, or the `upwell::transition` tracing events.

use std::fs;

use upwell_di::{Component as _, NodeAction};

use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, lock_test_guard, reject_hook_token,
    reset_controls, write_probe_config,
};
use crate::test_support::{CapturedEvent, capture_events};
use crate::{TransitionFailure, TransitionOutcome, TransitionPhase};

const SECRET: &str = "s3cr3t-probe-token";

/// A proposed token the rejecting hook formats into its error message.
const SECRET_TOKEN: i64 = 731_904_265;

fn assert_events_redacted(events: &[CapturedEvent], secret: &str) {
    for event in events {
        for (name, value) in &event.fields {
            assert!(
                !value.contains(secret),
                "tracing field '{name}' on '{}' leaks the config value",
                event.target
            );
        }
    }
}

fn transition_outcomes(events: &[CapturedEvent]) -> Vec<&str> {
    events
        .iter()
        .filter(|event| event.target == "upwell::transition")
        .filter_map(|event| event.field("outcome"))
        .collect()
}

#[tokio::test]
async fn noop_attempts_consume_distinct_attempt_ids() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (_dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    let first = runtime.reload_config().await.expect("first no-op succeeds");
    let second = runtime
        .reload_config()
        .await
        .expect("second no-op succeeds");

    assert_eq!(first.outcome(), &TransitionOutcome::NoOp);
    assert_eq!(second.outcome(), &TransitionOutcome::NoOp);
    assert_eq!(
        second.attempt().get(),
        first.attempt().get() + 1,
        "every no-op consumes the next attempt ID"
    );
    assert_eq!(first.runtime_generation(), second.runtime_generation());
    assert!(first.components().is_empty(), "a no-op plans nothing");
}

#[tokio::test]
async fn a_committed_reload_reports_affected_components_and_traces_them() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();
    let base_generation = runtime.generation();

    write_probe_config(&config_dir_of(&dir), true, 2);

    let (report, events) = capture_events(runtime.reload_config()).await;
    let report = report.expect("reload succeeds");
    let probe = report
        .component(ProbeComponent::ID)
        .expect("the probe consumer of the changed binding is affected");

    assert_eq!(
        report.outcome(),
        &TransitionOutcome::Committed {
            generation: runtime.generation(),
        }
    );
    assert_eq!(report.base_generation(), base_generation);
    assert_eq!(probe.action, NodeAction::Replace);
    assert!(!probe.reasons.is_empty(), "the replacement is explained");
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
            TransitionPhase::Construct,
            TransitionPhase::Hooks,
            TransitionPhase::Commit,
        ],
        "a committed reload times every phase in order"
    );

    let committed = events
        .iter()
        .find(|event| event.field("outcome") == Some("committed"))
        .expect("the commit is traced");
    let affected = events
        .iter()
        .find(|event| event.field("component") == Some(ProbeComponent::ID))
        .expect("the affected component is traced");

    assert_eq!(committed.target, "upwell::transition");
    assert_eq!(
        committed.field("attempt"),
        Some(report.attempt().get().to_string().as_str())
    );
    assert_eq!(affected.field("action"), Some("Replace"));
}

#[tokio::test]
async fn a_rejected_bind_reports_the_phase_and_category_without_the_value() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();
    let old_generation = runtime.generation();
    let old_config_generation = runtime.config_generation();

    fs::write(
        config_dir_of(&dir).join("application.toml"),
        format!("[probe]\nenabled = true\ntoken = \"{SECRET}\"\n"),
    )
    .expect("write a mistyped secret token");

    let (result, events) = capture_events(runtime.reload_config()).await;
    let error = result.expect_err("the mistyped token fails to bind");
    let crate::Error::TransitionRejected(rejection) = &error else {
        panic!("expected a rejected transition, got {error:?}");
    };
    let report = rejection.report();

    super::assert_rejected(
        report,
        TransitionPhase::Stage,
        TransitionFailure::ConfigBind {
            path: "probe".to_owned(),
            type_name: "ProbeConfig",
        },
    );
    assert_eq!(runtime.generation(), old_generation);
    assert_eq!(runtime.config_generation(), old_config_generation);
    assert_eq!(report.config_generation(), old_config_generation);
    assert!(!format!("{report:?}").contains(SECRET), "report Debug");
    assert!(!rejection.to_string().contains(SECRET), "rejection Display");
    assert!(!error.to_string().contains(SECRET), "app error Display");
    assert_eq!(transition_outcomes(&events), ["rejected"]);
    assert_events_redacted(&events, SECRET);

    #[cfg(feature = "serde")]
    {
        let json = serde_json::to_string(report).expect("the report serializes");

        assert!(!json.contains(SECRET), "serialized report");
    }
}

#[tokio::test]
async fn a_value_bearing_hook_rejection_never_reaches_the_report() {
    let _guard = lock_test_guard().await;
    let secret = SECRET_TOKEN.to_string();

    reset_controls().await;
    reject_hook_token(SECRET_TOKEN).await;

    let (dir, app) = build_probe_app(true, 1).await;
    let runtime = app.runtime();

    write_probe_config(&config_dir_of(&dir), true, SECRET_TOKEN);

    let (result, events) = capture_events(runtime.reload_config()).await;
    let error = result.expect_err("the hook rejects the secret token");
    let crate::Error::TransitionRejected(rejection) = &error else {
        panic!("expected a rejected transition, got {error:?}");
    };
    let report = rejection.report();

    assert!(
        format!("{:?}", rejection.error()).contains(&secret),
        "the unredacted hook error carries the value, so the checks below are meaningful"
    );
    super::assert_rejected(
        report,
        TransitionPhase::Hooks,
        TransitionFailure::HookRejected {
            component: ProbeComponent::NAME,
        },
    );
    assert!(
        report.component(ProbeComponent::ID).is_some(),
        "the attempted plan is reported"
    );
    assert!(!format!("{report:?}").contains(&secret), "report Debug");
    assert!(
        !rejection.to_string().contains(&secret),
        "rejection Display"
    );
    assert!(!error.to_string().contains(&secret), "app error Display");
    assert_events_redacted(&events, &secret);

    #[cfg(feature = "serde")]
    {
        let json = serde_json::to_string(report).expect("the report serializes");

        assert!(!json.contains(&secret), "serialized report");
    }
}
