//! Tooling diagnostics for seed-claim failures.

use super::FailureDetails;
use crate::registry::{SeedClaimError, SeedConsumer};

/// The diagnostic of a seed-claim failure found while preparing the application.
pub(super) fn seed_claim_failure(error: &SeedClaimError, phase: Option<String>) -> FailureDetails {
    match error {
        SeedClaimError::UnprovidedSeed {
            seed,
            scope_id,
            consumer,
            ..
        } => (
            "upwell/tooling-seed-unprovided",
            "A required context value has no seed claim for the scope its consumer reaches it in.",
            phase,
            consumer_resources(consumer, seed, *scope_id),
            Vec::new(),
            Some(
                "Claim the seed with the seed API of the protocol that opens the scope, or with `AppBuilder::provides_seed` for a scope the application opens.",
            ),
        ),
        SeedClaimError::OpenerWithoutSeed {
            seed,
            scope_id,
            consumer,
            ..
        } => (
            "upwell/tooling-seed-opener-missing",
            "One of the openers of a scope does not seed a value a consumer requires there.",
            phase,
            consumer_resources(consumer, seed, *scope_id),
            Vec::new(),
            Some("Claim the seed with the seed API of every opener of the scope."),
        ),
        SeedClaimError::ClaimWithoutSlot { seed, scope_id, .. } => (
            "upwell/tooling-seed-claim-without-slot",
            "A seed claim names a scope that has no seed slot for its type.",
            phase,
            vec![format!("type:{seed}"), format!("scope:{scope_id}")],
            Vec::new(),
            Some(
                "Declare the value in the claimed scope, for example with `#[context(scopes = [..])]`.",
            ),
        ),
    }
}

/// The consumer, the seed's type and the scope it is required from.
fn consumer_resources(
    consumer: &SeedConsumer,
    seed: &str,
    scope_id: upwell_core::ScopeId,
) -> Vec<String> {
    let consumer = match consumer {
        SeedConsumer::Component { id, .. } => crate::tooling::component_resource(id),
        SeedConsumer::Entry(key) => crate::tooling::entry_id(*key),
    };

    vec![
        consumer,
        format!("type:{seed}"),
        format!("scope:{scope_id}"),
    ]
}
