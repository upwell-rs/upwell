//! A switch whose candidate graph cannot be valid is rejected in planning with a message
//! that names the component and the unmet requirement, before anything is constructed.

use upwell_di::Component as _;

use super::super::super::{assert_rejected, rejected};
use super::super::fixture::lock_test_guard;
use super::faults::{Orphan, RIVAL_COMPONENT, RIVAL_PROVIDER, RivalAuthenticator};
use super::*;
use crate::{TransitionFailure, TransitionPhase};

/// Switches the hardening app with the rival provider registered to `mode` and returns
/// the rejection's message.
async fn rejected_switch(mode: &str) -> String {
    let dir = auth_config_dir("primary");

    reset_faults();

    let app = auth_builder::<()>(
        &config_dir_of(&dir),
        &PRIMARY_COMPONENT,
        &HOOKED_GATE_COMPONENT,
        &[&AUDIT_COMPONENT, &ORPHAN_COMPONENT, &RIVAL_COMPONENT],
    )
    .provider_descriptor(&RIVAL_PROVIDER)
    .build()
    .await
    .expect("the diagnostics app builds");
    let baseline = Baseline::capture(&app);

    write_auth_config(&config_dir_of(&dir), mode);

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("the invalid candidate is rejected");
    let (report, error) = rejected(error);

    assert_rejected(
        &report,
        TransitionPhase::Plan,
        TransitionFailure::GraphInvalid,
    );
    assert!(
        report.components().is_empty(),
        "{mode}: nothing was planned"
    );
    baseline.assert_intact(&app, mode).await;

    error.to_string()
}

#[tokio::test]
async fn a_newly_unsatisfiable_dependency_names_its_consumer_and_type() {
    let _guard = lock_test_guard().await;

    let message = rejected_switch("orphan").await;

    assert!(
        message.contains(Orphan::NAME),
        "names the consumer: {message}"
    );
    assert!(
        message.contains("Unprovided"),
        "names the missing type: {message}"
    );
}

#[tokio::test]
async fn newly_ambiguous_primaries_name_every_candidate() {
    let _guard = lock_test_guard().await;

    let message = rejected_switch("ambiguous").await;

    assert!(
        message.contains("Authenticator"),
        "names the trait: {message}"
    );
    assert!(
        message.contains("required by 'auth-gate'"),
        "names the consumer: {message}"
    );
    assert!(
        message.contains(RivalAuthenticator::ID) && message.contains(FallbackAuthenticator::ID),
        "names every tied candidate: {message}"
    );
    assert!(
        message.contains("distinct priorities"),
        "offers a fix that applies when both are already primary: {message}"
    );
}
