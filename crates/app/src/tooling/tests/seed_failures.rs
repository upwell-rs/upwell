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
            consumer: String::from("audit"),
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
fn a_duplicate_seed_claim_names_the_type_and_scope() {
    let diagnostic = diagnostic(crate::Error::SeedClaim(
        crate::SeedClaimError::DuplicateSeedClaim {
            seed: "app::Identity",
            scope: "Request",
            scope_id: scope(),
            first: "first",
            second: "second",
        },
    ));

    assert_eq!(diagnostic.code, "upwell/tooling-seed-claim-duplicate");
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
