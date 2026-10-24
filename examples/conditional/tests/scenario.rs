//! The worked example's acceptance (#299): switching `auth.mode` in both directions keeps the
//! live gate, reconstructs the fixed banner, carries the ledger's sessions through its
//! `#[replace]` recipe, and a replacement failure rolls the whole switch back while the audit
//! hooks see every proposed and committed change.

use std::sync::Arc;

use upwell::{
    AppError, Component, NodeAction, TransitionFailure, TransitionOutcome, TransitionPhase,
};
use upwell_example_conditional::{
    AuditEvent, AuthBanner, AuthMode, CustomAuthenticator, Demo, DemoError, LoginGate,
    SessionLedger,
};

fn plan(custom: NodeAction) -> AuditEvent {
    AuditEvent::Proposed(vec![
        (AuthBanner::ID, NodeAction::Replace),
        (CustomAuthenticator::ID, custom),
        (LoginGate::ID, NodeAction::RebindLive),
        (SessionLedger::ID, NodeAction::Replace),
    ])
}

#[tokio::test]
async fn switching_the_flag_moves_every_consumer_and_rolls_back_a_failed_replacement() {
    let demo = Demo::start(AuthMode::Default, 8)
        .await
        .expect("the demo starts");
    let gate = demo.gate();
    let banner = demo.banner();

    demo.ledger().open();
    demo.ledger().open();

    assert_eq!(gate.login("bob"), Some("default"));

    demo.switch(AuthMode::Custom, 8)
        .await
        .expect("switching to custom commits");

    let custom_banner = demo.banner();

    assert!(
        Arc::ptr_eq(&gate, &demo.gate()),
        "the live gate keeps its instance"
    );
    assert_eq!(
        gate.login("bob"),
        None,
        "the gate follows the custom provider"
    );
    assert_eq!(gate.login("custom:alice"), Some("custom"));
    assert!(
        !Arc::ptr_eq(&banner, &custom_banner),
        "the fixed banner is reconstructed"
    );
    assert_eq!(custom_banner.text(), "signed in with the custom scheme");
    assert_eq!(demo.ledger().scheme(), "custom");
    assert_eq!(
        demo.ledger().sessions(),
        2,
        "the replacement carried the sessions"
    );
    assert_eq!(
        demo.audit().take(),
        [
            plan(NodeAction::Add),
            AuditEvent::Committed {
                displaced: vec![AuthBanner::ID, SessionLedger::ID],
            },
        ]
    );

    demo.ledger().open();

    let generation = demo.runtime().generation();
    let ledger = demo.ledger();
    let rejected = demo
        .switch(AuthMode::Default, 1)
        .await
        .expect_err("three sessions do not fit a limit of one");
    let rejected_outcome = TransitionOutcome::Rejected {
        phase: TransitionPhase::Construct,
        failure: TransitionFailure::ConstructionFailed,
    };

    let DemoError::App(AppError::TransitionRejected(rejection)) = rejected else {
        panic!("a failed replacement rejects the transition: {rejected:?}");
    };

    assert_eq!(rejection.report().outcome(), &rejected_outcome);
    assert_eq!(
        demo.runtime().generation(),
        generation,
        "nothing was published"
    );
    assert!(
        Arc::ptr_eq(&ledger, &demo.ledger()),
        "the active ledger stays"
    );
    assert!(
        Arc::ptr_eq(&custom_banner, &demo.banner()),
        "the active banner stays"
    );
    assert_eq!(
        gate.login("bob"),
        None,
        "the gate still follows the custom provider"
    );
    assert_eq!(
        demo.audit().take(),
        [
            plan(NodeAction::Remove),
            AuditEvent::Aborted(rejected_outcome)
        ]
    );

    demo.switch(AuthMode::Default, 8)
        .await
        .expect("switching back with room for the sessions commits");

    assert!(
        Arc::ptr_eq(&gate, &demo.gate()),
        "the live gate keeps its instance"
    );
    assert_eq!(gate.login("bob"), Some("default"));
    assert!(!Arc::ptr_eq(&custom_banner, &demo.banner()));
    assert_eq!(demo.banner().text(), "signed in with the default scheme");
    assert_eq!(demo.ledger().scheme(), "default");
    assert_eq!(demo.ledger().sessions(), 3);
    assert_eq!(
        demo.audit().take(),
        [
            plan(NodeAction::Remove),
            AuditEvent::Committed {
                displaced: vec![AuthBanner::ID, SessionLedger::ID, CustomAuthenticator::ID],
            },
        ]
    );
}
