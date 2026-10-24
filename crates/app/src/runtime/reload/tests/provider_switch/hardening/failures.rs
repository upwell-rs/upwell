//! A fault at every phase of the Authenticator switch rejects the attempt in that phase
//! with its redacted category, leaves the complete previous generation active and usable,
//! and never blocks a later valid switch.

use upwell_di::Component as _;

use super::super::super::{assert_rejected, rejected};
use super::super::fixture::lock_test_guard;
use super::super::gate_hooks::Behavior;
use super::*;
use crate::{TransitionFailure, TransitionPhase};

/// One injected fault and where it must reject the attempt.
#[derive(Clone, Copy, Debug)]
enum Fault {
    UnparsableSource,
    UnbindableValue,
    FactExtractionPanic,
    UnsatisfiableCandidate,
    ProposalRejects,
    ProposalPanics,
    FactoryFails,
    FactoryPanics,
    ConfigHookRejects,
    ConfigHookPanics,
    DecisionRejects,
    DecisionPanics,
}

impl Fault {
    const ALL: [Self; 12] = [
        Self::UnparsableSource,
        Self::UnbindableValue,
        Self::FactExtractionPanic,
        Self::UnsatisfiableCandidate,
        Self::ProposalRejects,
        Self::ProposalPanics,
        Self::FactoryFails,
        Self::FactoryPanics,
        Self::ConfigHookRejects,
        Self::ConfigHookPanics,
        Self::DecisionRejects,
        Self::DecisionPanics,
    ];

    /// Arms the fault and writes the config the attempt stages.
    fn inject(self, dir: &TempDir) {
        let config_dir = config_dir_of(dir);
        let mut controls = Controls::default();

        match self {
            Self::UnparsableSource => {
                std::fs::write(config_dir.join("application.toml"), "[auth\n")
                    .expect("write unparsable config");

                return;
            }

            Self::UnbindableValue => {
                std::fs::write(config_dir.join("application.toml"), "[auth]\nmode = 5\n")
                    .expect("write unbindable config");

                return;
            }

            Self::UnsatisfiableCandidate => {
                write_auth_config(&config_dir, "orphan");

                return;
            }

            Self::FactExtractionPanic => panic_fact_extraction(true),
            Self::ProposalRejects => controls.gate_proposal = Behavior::Reject("not now"),
            Self::ProposalPanics => controls.gate_proposal = Behavior::Panic,
            Self::FactoryFails => fail_fallback_factory(true),
            Self::FactoryPanics => panic_fallback_factory(true),
            Self::ConfigHookRejects => set_config_hook_fault(ConfigHookFault::Reject),
            Self::ConfigHookPanics => set_config_hook_fault(ConfigHookFault::Panic),
            Self::DecisionRejects => controls.gate_decision = Behavior::Reject("not now"),
            Self::DecisionPanics => controls.gate_decision = Behavior::Panic,
        }

        set_controls(controls);
        write_auth_config(&config_dir, "fallback");
    }

    fn expected(self) -> (TransitionPhase, TransitionFailure) {
        let gate = AuthGate::ID;
        let sentinel = faults::Sentinel::ID;

        match self {
            Self::UnparsableSource => (TransitionPhase::Stage, TransitionFailure::ConfigLoad),
            Self::UnbindableValue => (
                TransitionPhase::Stage,
                TransitionFailure::ConfigBind {
                    path: String::from("auth"),
                    type_name: "AuthConfig",
                },
            ),
            Self::FactExtractionPanic => (TransitionPhase::Evaluate, TransitionFailure::Panicked),
            Self::UnsatisfiableCandidate => {
                (TransitionPhase::Plan, TransitionFailure::GraphInvalid)
            }
            Self::ProposalRejects => (
                TransitionPhase::Proposal,
                TransitionFailure::HookRejected { component: gate },
            ),
            Self::ProposalPanics => (
                TransitionPhase::Proposal,
                TransitionFailure::HookPanicked { component: gate },
            ),
            Self::FactoryFails => (
                TransitionPhase::Construct,
                TransitionFailure::ConstructionFailed,
            ),
            Self::FactoryPanics => (TransitionPhase::Construct, TransitionFailure::Panicked),
            Self::ConfigHookRejects => (
                TransitionPhase::Hooks,
                TransitionFailure::HookRejected {
                    component: sentinel,
                },
            ),
            Self::ConfigHookPanics => (
                TransitionPhase::Hooks,
                TransitionFailure::HookPanicked {
                    component: sentinel,
                },
            ),
            Self::DecisionRejects => (
                TransitionPhase::Decision,
                TransitionFailure::HookRejected { component: gate },
            ),
            Self::DecisionPanics => (
                TransitionPhase::Decision,
                TransitionFailure::HookPanicked { component: gate },
            ),
        }
    }
}

#[tokio::test]
async fn every_phase_fault_preserves_the_complete_previous_generation() {
    let _guard = lock_test_guard().await;

    for fault in Fault::ALL {
        let case = format!("{fault:?}");
        let (dir, app) = hardening_app().await;
        let baseline = Baseline::capture(&app);
        let (phase, failure) = fault.expected();

        fault.inject(&dir);

        let error = app
            .runtime()
            .reload_config()
            .await
            .expect_err(&format!("{case}: the faulted switch is rejected"));
        let (report, _) = rejected(error);

        assert_rejected(&report, phase, failure);
        assert!(
            report.displaced().is_empty(),
            "{case}: nothing is displaced"
        );
        baseline.assert_intact(&app, &case).await;
        assert_recovers(&dir, &app, &case).await;
    }

    reset_faults();
}

#[tokio::test]
async fn repeated_faults_never_accumulate_state_in_the_active_generation() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hardening_app().await;
    let baseline = Baseline::capture(&app);

    for round in 0..3 {
        for fault in Fault::ALL {
            let case = format!("round {round}, {fault:?}");

            fault.inject(&dir);

            assert!(
                app.runtime().reload_config().await.is_err(),
                "{case}: the faulted switch is rejected"
            );

            reset_faults();
            write_auth_config(&config_dir_of(&dir), "primary");
            baseline.assert_intact(&app, &case).await;
        }
    }

    assert_recovers(&dir, &app, "after every fault").await;
}
