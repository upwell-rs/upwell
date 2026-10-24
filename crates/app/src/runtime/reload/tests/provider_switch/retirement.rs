//! The #289 acceptance path: after an Authenticator switch commits, every displaced
//! singleton retires exactly once against its own instance, while pinned views and open
//! scopes keep using the retired instances and retirement failures stay isolated.

use std::sync::Arc;

use tempfile::TempDir;
use upwell_core::{ScopeId, StaticScope};
use upwell_di::{Component as _, ScopeContainer};

use super::fixture::{
    PrimaryAuthenticator, build_auth_app_from, config_dir_of, fail_fallback_factory,
    lock_test_guard, write_auth_config,
};
use super::gate::AuthGate;
use super::retirement_hooks::{
    AUDIT_ALWAYS, AUDIT_COMPONENT, RETIRING_GATE_COMPONENT, RETIRING_PRIMARY_COMPONENT,
    RetirementAudit, RetirementControls, SECRET_RETIREMENT_PANIC, retired, retired_by,
    set_retirement_controls,
};
use crate::test_support::capture_events;
use crate::{App, RetirementReason};

/// A child boundary opened directly over a pinned root, standing in for a protocol scope.
struct PinnedScope;

impl StaticScope for PinnedScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/pinned");
    const RANK: u8 = 50;
    const NAME: &'static str = "Pinned";
}

static PINNED_SCOPE: PinnedScope = PinnedScope;

async fn retiring_app(controls: RetirementControls) -> (TempDir, App<()>) {
    let built = build_auth_app_from(
        "primary",
        &RETIRING_PRIMARY_COMPONENT,
        &RETIRING_GATE_COMPONENT,
        &[&AUDIT_COMPONENT],
    )
    .await;

    fail_fallback_factory(false);
    set_retirement_controls(controls);

    built
}

fn address<T>(instance: &Arc<T>) -> usize {
    Arc::as_ptr(instance) as usize
}

#[tokio::test]
async fn displaced_components_retire_exactly_once_against_their_own_instances() {
    let _guard = lock_test_guard().await;
    let (dir, app) = retiring_app(RetirementControls::default()).await;
    let old_view = app.runtime().view();
    let old_primary = old_view
        .root()
        .get::<PrimaryAuthenticator>()
        .expect("the primary resolves before the switch");
    let old_gate = old_view
        .root()
        .get::<AuthGate>()
        .expect("the gate resolves before the switch");

    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the switch commits");
    let new_gate = app
        .runtime()
        .root()
        .get::<AuthGate>()
        .expect("the replacement gate resolves");
    let primary = retired_by(PrimaryAuthenticator::ID);
    let gate = retired_by(AuthGate::ID);

    assert_eq!(
        report.displaced(),
        [AuthGate::ID, PrimaryAuthenticator::ID],
        "the consumer retires before the provider it depends on"
    );
    assert_eq!(
        primary.len(),
        1,
        "the removed provider retires exactly once"
    );
    assert_eq!(gate.len(), 1, "the replaced consumer retires exactly once");
    assert_eq!(primary[0].retirement.component, PrimaryAuthenticator::ID);
    assert_eq!(primary[0].retirement.reason, RetirementReason::Removed);
    assert_eq!(gate[0].retirement.component, AuthGate::ID);
    assert_eq!(gate[0].retirement.reason, RetirementReason::Replaced);
    assert_eq!(
        primary[0].receiver,
        address(&old_primary),
        "the removed provider retires against its own instance"
    );
    assert_eq!(
        gate[0].receiver,
        address(&old_gate),
        "the replaced gate retires against the displaced instance"
    );
    assert_ne!(gate[0].receiver, address(&new_gate), "not the replacement");

    for retired in primary.iter().chain(&gate) {
        assert_eq!(retired.retirement.attempt, report.attempt());
        assert_eq!(retired.retirement.retired_generation, old_view.id());
        assert_eq!(
            retired.retirement.successor_generation,
            app.runtime().generation()
        );
    }

    assert!(
        retired_by(RetirementAudit::ID).is_empty(),
        "a retained component's targeted retirement hook never runs"
    );
    assert_eq!(
        retired_by(AUDIT_ALWAYS)
            .iter()
            .map(|retired| retired.retirement.component)
            .collect::<Vec<_>>(),
        [AuthGate::ID, PrimaryAuthenticator::ID],
        "an always hook observes every retirement in retirement order"
    );
}

#[tokio::test]
async fn pinned_views_and_open_scopes_keep_using_retired_instances() {
    let _guard = lock_test_guard().await;
    let (dir, app) = retiring_app(RetirementControls::default()).await;
    let old_view = app.runtime().view();
    let old_scope = ScopeContainer::open_child(
        &PINNED_SCOPE,
        Arc::clone(old_view.root()),
        Arc::clone(old_view.scopes()),
        &[],
        Vec::new(),
    )
    .await
    .expect("a child scope opens over the old root");
    let old_primary = address(
        &old_view
            .root()
            .get::<PrimaryAuthenticator>()
            .expect("the primary resolves before the switch"),
    );

    write_auth_config(&config_dir_of(&dir), "fallback");
    app.runtime()
        .reload_config()
        .await
        .expect("the switch commits");

    let pinned_primary = old_view
        .root()
        .get::<PrimaryAuthenticator>()
        .expect("the pinned view still resolves the retired provider");
    let scoped_gate = old_scope
        .get::<AuthGate>()
        .expect("the open scope still resolves the retired gate");

    assert_eq!(retired().len(), 4, "both retirements ran before the checks");
    assert_eq!(address(&pinned_primary), old_primary);
    assert_eq!(pinned_primary.observed_mode, "primary");
    assert_eq!(scoped_gate.scheme, "primary");
    assert!(
        app.runtime().root().get::<PrimaryAuthenticator>().is_none(),
        "the published generation no longer has the provider"
    );
}

#[tokio::test]
async fn retirement_failures_are_isolated_and_traced_without_their_payload() {
    let _guard = lock_test_guard().await;
    let (dir, app) = retiring_app(RetirementControls {
        failing: Some(AuthGate::ID),
        panicking: Some(PrimaryAuthenticator::ID),
    })
    .await;
    let generation = app.runtime().generation().get();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let (result, events) = capture_events(app.runtime().reload_config()).await;
    let report = result.expect("a failing retirement cannot reject a committed attempt");
    let failures = events
        .iter()
        .filter(|event| event.field("message") == Some("component retirement hook failed"))
        .map(|event| (event.field("component"), event.field("outcome")))
        .collect::<Vec<_>>();

    assert!(report.is_committed());
    assert_eq!(app.runtime().generation().get(), generation + 1);
    assert_eq!(
        retired_by(AUDIT_ALWAYS).len(),
        2,
        "a failing retirement does not stop later retirements"
    );
    assert_eq!(
        failures,
        [
            (Some(AuthGate::ID), Some("failed")),
            (Some(PrimaryAuthenticator::ID), Some("panicked")),
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| !format!("{event:?}").contains(SECRET_RETIREMENT_PANIC)),
        "the panic payload is never traced"
    );

    write_auth_config(&config_dir_of(&dir), "primary");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("a failed retirement cannot block a later reload");

    assert!(report.is_committed());
    assert_eq!(app.runtime().generation().get(), generation + 2);
}

#[tokio::test]
async fn rejected_and_no_op_attempts_retire_nothing() {
    let _guard = lock_test_guard().await;
    let (dir, app) = retiring_app(RetirementControls::default()).await;

    let no_op = app
        .runtime()
        .reload_config()
        .await
        .expect("an unchanged source is a no-op");

    fail_fallback_factory(true);
    write_auth_config(&config_dir_of(&dir), "fallback");

    let rejected = app
        .runtime()
        .reload_config()
        .await
        .expect_err("a failing candidate factory rejects the switch");

    fail_fallback_factory(false);

    let crate::Error::TransitionRejected(rejection) = rejected else {
        panic!("a construction failure is a transition rejection");
    };

    assert!(no_op.displaced().is_empty());
    assert!(
        rejection.report().displaced().is_empty(),
        "nothing was displaced because nothing was published"
    );
    assert!(retired().is_empty(), "no retirement hook ran");
}
