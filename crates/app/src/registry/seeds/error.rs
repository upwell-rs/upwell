//! What seed-claim validation reports.

use upwell_core::{EntryKey, ScopeId};

/// The component or entry that requires a seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeedConsumer {
    /// A component placement, by its stable identity and display name.
    Component {
        /// The component's stable identity.
        id: &'static str,
        /// The component's display name.
        name: &'static str,
    },
    /// An entry, by its stable key.
    Entry(EntryKey),
}

/// What [`AppRegistry::validate_seed_claims`](super::AppRegistry) reports.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SeedClaimError {
    /// A required context value has no seed claim for the scope the consumer reaches it in.
    #[error(
        "'{consumer}' requires '{seed}' from scope '{scope}', but nothing claims to seed it \
         there; declare it with the seed API of the protocol that opens the scope, or with \
         `AppBuilder::provides_seed` for a scope the application opens"
    )]
    UnprovidedSeed {
        /// The seeded value's type name.
        seed: &'static str,
        /// The scope expected to provide it.
        scope: &'static str,
        /// The scope's identity.
        scope_id: ScopeId,
        /// The component or entry that requires it.
        consumer: SeedConsumer,
    },

    /// One of several openers of a scope does not seed a value a consumer requires there, so
    /// every instance it opens lacks it, whatever the other openers seed.
    #[error(
        "'{consumer}' requires '{seed}' from scope '{scope}', but '{opener}', which opens that \
         scope, does not seed it; declare it with that opener's seed API"
    )]
    OpenerWithoutSeed {
        /// The seeded value's type name.
        seed: &'static str,
        /// The scope expected to provide it.
        scope: &'static str,
        /// The scope's identity.
        scope_id: ScopeId,
        /// The component or entry that requires it.
        consumer: SeedConsumer,
        /// The opener that does not seed it.
        opener: String,
    },

    /// A claim names a type the claimed scope has no factory-less slot for, so every opening
    /// that fills it would be rejected for a seed without a slot.
    #[error(
        "'{claimant}' claims to seed '{seed}' into scope '{scope}', which has no seed slot for \
         it; declare it there, for example with `#[context(scopes = [..])]`"
    )]
    ClaimWithoutSlot {
        /// The claimed value's type name.
        seed: &'static str,
        /// The claimed scope.
        scope: &'static str,
        /// The claimed scope's identity.
        scope_id: ScopeId,
        /// Who claims it.
        claimant: String,
    },
}

impl std::fmt::Display for SeedConsumer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Component { name, .. } => formatter.write_str(name),
            Self::Entry(key) => write!(formatter, "entry {key}"),
        }
    }
}
