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
            vec![
                consumer_resource(consumer),
                format!("type:{seed}"),
                format!("scope:{scope_id}"),
            ],
            Vec::new(),
            Some(
                "Claim the seed with the protocol's seed API (`request_seed`, `StompConfig::provides_seed`) or `AppBuilder::provides_seed`.",
            ),
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

/// The projection resource of `consumer`: a component by its id, an entry by its key.
fn consumer_resource(consumer: &SeedConsumer) -> String {
    match consumer {
        SeedConsumer::Component { id, .. } => crate::tooling::component_resource(id),
        SeedConsumer::Entry(key) => crate::tooling::entry_id(*key),
    }
}
