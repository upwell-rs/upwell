//! Tooling diagnostics for seed-claim failures.

use super::FailureDetails;
use crate::registry::SeedClaimError;

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
                crate::tooling::component_resource(consumer),
                format!("type:{seed}"),
                format!("scope:{scope_id}"),
            ],
            Vec::new(),
            Some(
                "Claim the seed with the protocol's seed API (`request_seed`, `StompConfig::provides_seed`) or `AppBuilder::provides_seed`.",
            ),
        ),
        SeedClaimError::DuplicateSeedClaim { seed, scope_id, .. } => (
            "upwell/tooling-seed-claim-duplicate",
            "Two claimants guarantee the same seed in one scope.",
            phase,
            vec![format!("type:{seed}"), format!("scope:{scope_id}")],
            Vec::new(),
            Some("Keep one seed source per scope."),
        ),
    }
}
