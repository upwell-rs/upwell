//! Tests for the transactional config-and-graph reload entry point.
//!
//! Each test drives [`AppRuntime::reload_config`](crate::AppRuntime::reload_config)
//! over a file-backed app with one condition-gated component whose factory injects
//! `Cfg<T>` and whose `ConfigReload` hook records the proposed value, so the tests can
//! prove which store each stage of the transaction resolved through — and that every
//! failure or cancellation before the terminal commit leaves the complete previous
//! generation, config values included, active.

mod callback_inputs;
mod cancellation;
mod candidate_failure;
mod concurrency;
mod condition_panic;
mod fixture;
mod hook_rejection;
mod invalidation;
mod manager_consumer;
mod noop;
mod provider_switch;
mod publication;
mod replacement;
mod report;
mod runtime_reloader;
mod trigger;

use crate::{RuntimeTransitionReport, TransitionFailure, TransitionOutcome, TransitionPhase};

/// Splits a rejected reload's error into its redacted report and the original error.
fn rejected(error: crate::Error) -> (RuntimeTransitionReport, crate::Error) {
    match error {
        crate::Error::TransitionRejected(rejection) => rejection.into_parts(),
        other => panic!("expected a rejected transition, got {other:?}"),
    }
}

/// Asserts that a rejected report names the failing phase and redacted category.
fn assert_rejected(
    report: &RuntimeTransitionReport,
    phase: TransitionPhase,
    failure: TransitionFailure,
) {
    assert_eq!(
        report.outcome(),
        &TransitionOutcome::Rejected { phase, failure },
        "the report names the rejecting phase and category"
    );
    assert_eq!(
        report.runtime_generation(),
        report.base_generation(),
        "a rejected attempt leaves the base generation current"
    );
}
