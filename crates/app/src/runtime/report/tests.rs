use upwell_config::ConfigReloadError;
use upwell_core::RuntimeGenerationId;

use super::*;
use crate::test_support::capture_events;

const SECRET: &str = "s3cr3t-config-value";

fn recorder() -> TransitionRecorder {
    TransitionRecorder::begin(TransitionAttemptId::new(7), RuntimeGenerationId::new(3), 11)
}

fn rejection(result: crate::Result<RuntimeTransitionReport>) -> Box<TransitionRejection> {
    match result {
        Err(crate::Error::TransitionRejected(rejection)) => rejection,
        other => panic!("expected a transition rejection, got {other:?}"),
    }
}

#[test]
fn phases_have_stable_snake_case_names_in_execution_order() {
    let phases = [
        TransitionPhase::Stage,
        TransitionPhase::Evaluate,
        TransitionPhase::Plan,
        TransitionPhase::Construct,
        TransitionPhase::Hooks,
        TransitionPhase::Commit,
    ];

    let names = phases.map(TransitionPhase::as_str);

    assert_eq!(
        names,
        ["stage", "evaluate", "plan", "construct", "hooks", "commit"]
    );
    assert!(phases.is_sorted(), "phases order by execution");
}

#[test]
fn a_noop_attempt_reports_its_attempt_and_keeps_the_base_generation() {
    let mut recorder = recorder();

    recorder.enter(TransitionPhase::Stage);

    let report = recorder
        .finish(Ok(TransitionCompletion::NoOp))
        .expect("a no-op finishes successfully");

    assert_eq!(report.attempt().get(), 7);
    assert_eq!(report.outcome(), &TransitionOutcome::NoOp);
    assert!(!report.is_committed());
    assert_eq!(report.runtime_generation(), RuntimeGenerationId::new(3));
    assert_eq!(
        report.config_generation(),
        11,
        "a no-op keeps the config generation"
    );
    assert_eq!(
        report
            .phases()
            .iter()
            .map(|timing| timing.phase)
            .collect::<Vec<_>>(),
        [TransitionPhase::Stage]
    );
}

#[test]
fn a_committed_attempt_reports_the_published_generations() {
    let mut recorder = recorder();

    recorder.enter(TransitionPhase::Stage);
    recorder.enter(TransitionPhase::Commit);

    let report = recorder
        .finish(Ok(TransitionCompletion::Committed {
            generation: RuntimeGenerationId::new(4),
            config_generation: 12,
        }))
        .expect("a commit finishes successfully");

    assert!(report.is_committed());
    assert_eq!(report.runtime_generation(), RuntimeGenerationId::new(4));
    assert_eq!(report.base_generation(), RuntimeGenerationId::new(3));
    assert_eq!(report.config_generation(), 12);
    assert_eq!(report.phases().len(), 2, "both entered phases are timed");
}

#[test]
fn a_rejected_attempt_reports_the_failing_phase_and_redacted_category() {
    let mut recorder = recorder();
    let error = crate::Error::ConfigReload(Box::new(ConfigReloadError::Panicked));

    recorder.enter(TransitionPhase::Stage);

    let rejection = rejection(recorder.finish(Err(error)));
    let report = rejection.report();

    assert_eq!(
        report.outcome(),
        &TransitionOutcome::Rejected {
            phase: TransitionPhase::Stage,
            failure: TransitionFailure::Panicked,
        }
    );
    assert_eq!(report.runtime_generation(), RuntimeGenerationId::new(3));
    assert_eq!(
        rejection.to_string(),
        "runtime transition attempt 7 was rejected during stage: an application extension \
         point panicked"
    );
    assert!(
        matches!(
            rejection.error(),
            crate::Error::ConfigReload(error) if matches!(**error, ConfigReloadError::Panicked)
        ),
        "the original error stays reachable"
    );
}

#[test]
fn classification_uses_variants_and_never_error_messages() {
    let secret_error = crate::Error::Di(upwell_di::Error::Other(SECRET.into()));

    let construct = TransitionFailure::classify(TransitionPhase::Construct, &secret_error);
    let plan = TransitionFailure::classify(TransitionPhase::Plan, &secret_error);
    let restart = TransitionFailure::classify(
        TransitionPhase::Plan,
        &crate::Error::RestartRequired(crate::RestartRequired {
            component: "component-a",
            required: Some(upwell_di::NodeAction::Replace),
            reason: crate::RestartReason::FactoryUnavailable,
        }),
    );

    assert_eq!(construct, TransitionFailure::ConstructionFailed);
    assert_eq!(plan, TransitionFailure::GraphInvalid);
    assert_eq!(
        restart,
        TransitionFailure::RestartRequired {
            component: "component-a",
            reason: crate::RestartReason::FactoryUnavailable,
        }
    );

    for failure in [construct, plan, restart] {
        assert!(!failure.to_string().contains(SECRET));
        assert!(!format!("{failure:?}").contains(SECRET));
    }
}

#[tokio::test]
async fn a_dropped_recorder_emits_a_cancelled_event() {
    let ((), events) = capture_events(async {
        let mut recorder = recorder();

        recorder.enter(TransitionPhase::Construct);

        drop(recorder);
    })
    .await;

    let cancelled = events
        .iter()
        .find(|event| event.field("outcome") == Some("cancelled"))
        .expect("the dropped recorder emits a cancellation event");

    assert_eq!(cancelled.target, "upwell::transition");
    assert_eq!(cancelled.field("attempt"), Some("7"));
    assert_eq!(cancelled.field("phase"), Some("construct"));
}

#[tokio::test]
async fn a_finished_recorder_emits_one_terminal_event_and_no_cancellation() {
    let (_, events) = capture_events(async {
        recorder()
            .finish(Ok(TransitionCompletion::NoOp))
            .expect("a no-op finishes successfully")
    })
    .await;

    let outcomes = events
        .iter()
        .filter_map(|event| event.field("outcome"))
        .collect::<Vec<_>>();

    assert_eq!(outcomes, ["no_op"]);
}

#[cfg(feature = "serde")]
#[test]
fn a_rejected_report_serializes_its_outcome_without_error_text() {
    let mut recorder = recorder();
    let error = crate::Error::Di(upwell_di::Error::Other(SECRET.into()));

    recorder.enter(TransitionPhase::Construct);

    let rejection = rejection(recorder.finish(Err(error)));
    let json = serde_json::to_value(rejection.report()).expect("the report serializes");

    assert_eq!(json["attempt"], 7);
    assert_eq!(json["base_generation"], 3);
    assert_eq!(json["outcome"], "rejected");
    assert_eq!(json["phase"], "construct");
    assert_eq!(json["failure"]["category"], "construction_failed");
    assert!(!json.to_string().contains(SECRET));
}
