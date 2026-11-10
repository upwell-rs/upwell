//! Diagnostics of context values and seed claims.

use super::*;

/// The first diagnostic of `error` failing preparation.
fn diagnostic(error: crate::Error) -> upwell_tooling_schema::Diagnostic {
    ToolingProbeError::Lifecycle(crate::PhaseError::new(
        crate::LifecyclePhase::Prepare,
        error,
    ))
    .failure()
    .diagnostics
    .remove(0)
}

fn scope() -> upwell_core::ScopeId {
    upwell_core::ScopeId::new("fixture/request").expect("valid scope identity")
}

#[test]
fn an_unprovided_seed_names_its_consumer_type_and_scope() {
    let diagnostic = diagnostic(crate::Error::SeedClaim(
        crate::SeedClaimError::UnprovidedSeed {
            seed: "app::Identity",
            scope: "Request",
            scope_id: scope(),
            consumer: crate::SeedConsumer::Component {
                id: "audit",
                name: "Audit",
            },
        },
    ));

    assert_eq!(diagnostic.code, "upwell/tooling-seed-unprovided");
    assert_eq!(
        diagnostic.resources,
        [
            "component:audit",
            "type:app::Identity",
            "scope:fixture/request"
        ]
    );
}

#[test]
fn an_entry_consumer_is_named_as_an_entry() {
    let key = upwell_core::EntryKey::new("http/greet.get").expect("a valid entry key");
    let diagnostic = diagnostic(crate::Error::SeedClaim(
        crate::SeedClaimError::UnprovidedSeed {
            seed: "app::Identity",
            scope: "Request",
            scope_id: scope(),
            consumer: crate::SeedConsumer::Entry(key),
        },
    ));

    assert_eq!(diagnostic.resources[0], "entry:http/greet.get");
}

#[test]
fn an_opener_without_the_seed_names_its_consumer_type_and_scope() {
    let diagnostic = diagnostic(crate::Error::SeedClaim(
        crate::SeedClaimError::OpenerWithoutSeed {
            seed: "app::Identity",
            scope: "Request",
            scope_id: scope(),
            consumer: crate::SeedConsumer::Component {
                id: "audit",
                name: "Audit",
            },
            opener: String::from("JsonWs at /json"),
        },
    ));

    assert_eq!(diagnostic.code, "upwell/tooling-seed-opener-missing");
    assert!(
        diagnostic.message.contains("JsonWs at /json"),
        "{}",
        diagnostic.message
    );
    assert_eq!(
        diagnostic.resources,
        [
            "component:audit",
            "type:app::Identity",
            "scope:fixture/request",
            "opener:JsonWs at /json"
        ]
    );
}

#[test]
fn a_claim_without_a_slot_names_the_type_and_scope() {
    let diagnostic = diagnostic(crate::Error::SeedClaim(
        crate::SeedClaimError::ClaimWithoutSlot {
            seed: "app::Identity",
            scope: "Request",
            scope_id: scope(),
            claimant: String::from("app::HeaderIdentity"),
        },
    ));

    assert_eq!(diagnostic.code, "upwell/tooling-seed-claim-without-slot");
    assert_eq!(
        diagnostic.resources,
        ["type:app::Identity", "scope:fixture/request"]
    );
}

#[test]
fn an_invalid_context_names_the_context_and_its_scope() {
    let diagnostic = diagnostic(crate::Error::Di(upwell_di::Error::InvalidContext {
        component_id: String::from("identity"),
        scope: "Request",
        scope_id: scope(),
    }));

    assert_eq!(diagnostic.code, "upwell/tooling-context-invalid");
    assert_eq!(
        diagnostic.resources,
        ["component:identity", "scope:fixture/request"]
    );
}
