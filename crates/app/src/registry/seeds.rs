//! Seed claims: which values a protocol or the application guarantees to seed into a scope,
//! and the preparation check that every required context value is claimed.
//!
//! DI core knows only that a value is seeded from outside, never how; the claims say who
//! seeds it. A protocol claims its framework seeds when it registers, an endpoint claims what
//! its options make it produce, and the application claims what it seeds into scopes it
//! opens itself. The check covers `#[context]` values and every type something claims; a
//! plain factory-less component no one claims is left to fail when it is resolved, as before.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{
    DependencyDescriptor, EntryKey, ResolutionMode, Scope, ScopeId, ScopeKey, TypeDescriptor,
};
use upwell_di::ComponentDescriptor;

use super::AppRegistry;
use crate::scope::PreparedScopeTopology;

/// The claimant of the seeds the application claims with
/// [`AppBuilder::provides_seed`](crate::AppBuilder::provides_seed).
pub const APPLICATION_CLAIMANT: &str = "application";

/// A guarantee that `claimant` seeds a value of type `ty` into every instance of `scope` it
/// opens, so a component of that scope or below it may require the value.
#[derive(Clone, Copy, Debug)]
pub struct SeedClaim {
    /// The seeded value's type.
    pub ty: TypeDescriptor,
    /// The scope whose instances receive the seed.
    pub scope: ScopeKey,
    /// Who seeds it, for diagnostics: a protocol, an endpoint option or the application.
    pub claimant: &'static str,
}

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

/// What [`AppRegistry::validate_seed_claims`] reports.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SeedClaimError {
    /// A required context value has no seed claim for the scope the consumer reaches it in.
    #[error(
        "'{consumer}' requires '{seed}' from scope '{scope}', but nothing claims to seed it there; \
         declare it with the protocol's seed API (`request_seed`, `StompConfig::provides_seed`) \
         or `AppBuilder::provides_seed` for a scope the application opens"
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

    /// A claim names a type the claimed scope has no factory-less slot for, so every opening
    /// that fills it would be rejected for a seed without a slot.
    #[error(
        "'{claimant}' claims to seed '{seed}' into scope '{scope}', which has no seed slot for it; \
         declare it there, for example with `#[context(scopes = [..])]`"
    )]
    ClaimWithoutSlot {
        /// The claimed value's type name.
        seed: &'static str,
        /// The claimed scope.
        scope: &'static str,
        /// The claimed scope's identity.
        scope_id: ScopeId,
        /// Who claims it.
        claimant: &'static str,
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

impl SeedClaim {
    /// `claimant` seeds `T` into every instance of `S`.
    pub fn of<T: 'static, S: Scope>(claimant: &'static str) -> Self {
        Self {
            ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
            scope: S::KEY,
            claimant,
        }
    }

    fn key(&self) -> (TypeId, ScopeId) {
        (self.ty.type_id, self.scope.id())
    }
}

impl AppRegistry {
    /// Records `claim`, once per claimant: an endpoint registered twice claims once.
    pub fn claim_seed(&mut self, claim: SeedClaim) {
        let known = self
            .seed_claims
            .iter()
            .any(|existing| existing.key() == claim.key() && existing.claimant == claim.claimant);

        if !known {
            self.seed_claims.push(claim);
        }
    }

    /// Rejects a claim for a scope without a seed slot of its type, and a required dependency
    /// on a checked seed whose placement on the consumer's chain no one claims. A checked seed
    /// is a `#[context]` value or a type something claims.
    ///
    /// Several claimants of one `(type, scope)` pair are accepted: each describes an opener
    /// that fills its own instances of the scope, such as two WebSocket protocols each seeding
    /// their connections. Two fills of one opening are rejected where the opening is assembled
    /// (two request seeders of one type fail the axum build) or when the instance opens
    /// (`DuplicateSeed`).
    pub(crate) fn validate_seed_claims(
        &self,
        components: &[ComponentDescriptor],
        topology: &PreparedScopeTopology,
    ) -> Result<(), SeedClaimError> {
        let claimed = self.claimed_scopes(components)?;
        let seeds = SeedPlacements::of(components, &claimed);

        if seeds.is_empty() {
            return Ok(());
        }

        for component in components {
            for dependency in component.dependencies() {
                let consumer = SeedConsumer::Component {
                    id: component.id,
                    name: component.name,
                };

                seeds.check(&dependency, component.scope, consumer, &claimed, topology)?;
            }
        }

        for entry in &self.entries {
            for dependency in (entry.targets)() {
                let consumer = SeedConsumer::Entry(entry.key);

                seeds.check(&dependency, entry.scope, consumer, &claimed, topology)?;
            }
        }

        Ok(())
    }

    /// The claimed `(type, scope)` pairs, rejecting a claim whose scope has no factory-less
    /// slot of its type among `components`.
    fn claimed_scopes(
        &self,
        components: &[ComponentDescriptor],
    ) -> Result<HashSet<(TypeId, ScopeId)>, SeedClaimError> {
        let slots: HashSet<(TypeId, ScopeId)> = components
            .iter()
            .filter(|component| component.effective_factory().ok().flatten().is_none())
            .map(|component| (component.ty.type_id, component.scope.id()))
            .collect();

        for claim in &self.seed_claims {
            if !slots.contains(&claim.key()) {
                return Err(SeedClaimError::ClaimWithoutSlot {
                    seed: (claim.ty.type_name)(),
                    scope: claim.scope.name(),
                    scope_id: claim.scope.id(),
                    claimant: claim.claimant,
                });
            }
        }

        Ok(self.seed_claims.iter().map(SeedClaim::key).collect())
    }
}

/// The scopes each checked seed is placed in.
struct SeedPlacements(HashMap<TypeId, Vec<ScopeKey>>);

impl SeedPlacements {
    /// The factory-less placements of every `#[context]` value and every claimed type.
    fn of(components: &[ComponentDescriptor], claimed: &HashSet<(TypeId, ScopeId)>) -> Self {
        let claimed_types: HashSet<TypeId> = claimed.iter().map(|(ty, _)| *ty).collect();
        let mut placements: HashMap<TypeId, Vec<ScopeKey>> = HashMap::new();

        for component in components {
            let type_id = component.ty.type_id;
            let checked = component.construction.is_context() || claimed_types.contains(&type_id);
            let seeded = component.effective_factory().ok().flatten().is_none();

            if checked && seeded {
                placements.entry(type_id).or_default().push(component.scope);
            }
        }

        Self(placements)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fails when `dependency`, required by `consumer` in `scope`, reaches an unclaimed
    /// placement of a checked seed.
    fn check(
        &self,
        dependency: &DependencyDescriptor,
        scope: ScopeKey,
        consumer: SeedConsumer,
        claimed: &HashSet<(TypeId, ScopeId)>,
        topology: &PreparedScopeTopology,
    ) -> Result<(), SeedClaimError> {
        let required = !dependency.optional
            && !dependency.config
            && matches!(
                dependency.resolution,
                ResolutionMode::Eager | ResolutionMode::Deferred
            );
        let Some(placements) = self.0.get(&dependency.ty.type_id) else {
            return Ok(());
        };

        if !required {
            return Ok(());
        }

        let reached = placements
            .iter()
            .find(|placement| topology.is_reachable(&scope.id(), &placement.id()));

        match reached {
            Some(placement) if !claimed.contains(&(dependency.ty.type_id, placement.id())) => {
                Err(SeedClaimError::UnprovidedSeed {
                    seed: (dependency.ty.type_name)(),
                    scope: placement.name(),
                    scope_id: placement.id(),
                    consumer,
                })
            }

            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
