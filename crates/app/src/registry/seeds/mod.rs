//! Seed claims: which values a protocol or the application guarantees to seed into a scope,
//! and the preparation check that every required context value is claimed.
//!
//! DI core knows only that a value is seeded from outside, never how; the claims say who
//! seeds it. A scope one opener opens, such as the HTTP request scope or a scope the
//! application opens itself, is covered by plain [`SeedClaim`]s. A scope several openers open,
//! such as the WebSocket connection scope every endpoint opens, is covered per opener: each
//! declares the seeds it fills with [`OpenerSeeds`], and a value is provided there only when
//! every opener seeds it, since an instance one opener opens never sees what another seeds.
//! The check covers `#[context]` values and every claimed type; a plain factory-less component
//! no one claims is left to fail when it is resolved, as before.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{DependencyDescriptor, ResolutionMode, Scope, ScopeId, ScopeKey, TypeDescriptor};
use upwell_di::ComponentDescriptor;

use super::AppRegistry;
use crate::scope::PreparedScopeTopology;

mod error;

pub use error::{SeedClaimError, SeedConsumer};

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

/// One opener of a scope several openers share, such as one WebSocket endpoint opening the
/// connection scope, and every seed it fills in each instance it opens.
#[derive(Clone, Debug)]
pub struct OpenerSeeds {
    /// The opener, for diagnostics, such as a protocol and the path it is mounted at.
    pub opener: String,
    /// The scope it opens.
    pub scope: ScopeKey,
    /// The seeds it fills in every instance.
    pub seeds: Vec<TypeDescriptor>,
}

/// Which `(type, scope)` pairs are provided: by plain claims, or by every opener of a scope
/// that declares its openers.
struct Provided<'a> {
    claimed: HashSet<(TypeId, ScopeId)>,
    openers: HashMap<ScopeId, Vec<&'a OpenerSeeds>>,
}

/// Whether one `(type, scope)` pair is provided.
enum Coverage<'a> {
    /// A claim covers it, or every declared opener of the scope seeds it.
    Provided,
    /// The scope declares its openers, and this one does not seed it.
    MissingIn(&'a OpenerSeeds),
    /// The scope declares no openers, and no claim covers it.
    Unclaimed,
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

impl OpenerSeeds {
    fn seeds(&self, ty: TypeId) -> bool {
        self.seeds.iter().any(|seed| seed.type_id == ty)
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

    /// Declares `opener` as one of the openers of its scope. Once a scope has a declared
    /// opener, a value is provided there only when every declared opener seeds it.
    pub fn declare_opener(&mut self, opener: OpenerSeeds) {
        self.scope_openers.push(opener);
    }

    /// Rejects a claim for a scope without a seed slot of its type, and a required dependency
    /// on a checked seed whose placement on the consumer's chain is not provided. A checked seed
    /// is a `#[context]` value or a type something claims.
    ///
    /// Several claimants of one `(type, scope)` pair are accepted: each describes an opener that
    /// fills its own instances. Where a scope declares its openers, every one of them must seed
    /// a required value, so one opener's seed never vouches for another's instances. Two fills
    /// of one opening are rejected where the opening is assembled, or when the instance opens
    /// (`DuplicateSeed`).
    pub(crate) fn validate_seed_claims(
        &self,
        components: &[ComponentDescriptor],
        topology: &PreparedScopeTopology,
    ) -> Result<(), SeedClaimError> {
        let provided = self.provided(components)?;
        let seeds = SeedPlacements::of(components, &provided);

        if seeds.is_empty() {
            return Ok(());
        }

        for component in components {
            for dependency in component.dependencies() {
                let consumer = SeedConsumer::Component {
                    id: component.id,
                    name: component.name,
                };

                seeds.check(&dependency, component.scope, consumer, &provided, topology)?;
            }
        }

        for entry in &self.entries {
            for dependency in (entry.targets)() {
                let consumer = SeedConsumer::Entry(entry.key);

                seeds.check(&dependency, entry.scope, consumer, &provided, topology)?;
            }
        }

        Ok(())
    }

    /// What the claims and declared openers provide, rejecting one that names a scope with no
    /// factory-less slot of its type among `components`.
    fn provided(&self, components: &[ComponentDescriptor]) -> Result<Provided<'_>, SeedClaimError> {
        let slots: HashSet<(TypeId, ScopeId)> = components
            .iter()
            .filter(|component| component.effective_factory().ok().flatten().is_none())
            .map(|component| (component.ty.type_id, component.scope.id()))
            .collect();
        let claims = self
            .seed_claims
            .iter()
            .map(|claim| (claim.ty, claim.scope, claim.claimant));
        let opened = self.scope_openers.iter().flat_map(|opener| {
            opener
                .seeds
                .iter()
                .map(|seed| (*seed, opener.scope, opener.opener.as_str()))
        });
        let mut openers: HashMap<ScopeId, Vec<&OpenerSeeds>> = HashMap::new();

        for (ty, scope, claimant) in claims.chain(opened) {
            if !slots.contains(&(ty.type_id, scope.id())) {
                return Err(SeedClaimError::ClaimWithoutSlot {
                    seed: (ty.type_name)(),
                    scope: scope.name(),
                    scope_id: scope.id(),
                    claimant: claimant.to_owned(),
                });
            }
        }

        for opener in &self.scope_openers {
            openers.entry(opener.scope.id()).or_default().push(opener);
        }

        Ok(Provided {
            claimed: self.seed_claims.iter().map(SeedClaim::key).collect(),
            openers,
        })
    }
}

impl Provided<'_> {
    /// Whether anything claims to seed `ty` into some scope.
    fn claims_type(&self, ty: TypeId) -> bool {
        self.claimed.iter().any(|(claimed, _)| *claimed == ty)
            || self
                .openers
                .values()
                .flatten()
                .any(|opener| opener.seeds(ty))
    }

    /// Whether `ty` is provided in `scope`.
    fn coverage(&self, ty: TypeId, scope: ScopeId) -> Coverage<'_> {
        let Some(openers) = self.openers.get(&scope) else {
            return match self.claimed.contains(&(ty, scope)) {
                true => Coverage::Provided,
                false => Coverage::Unclaimed,
            };
        };

        match openers.iter().find(|opener| !opener.seeds(ty)) {
            Some(opener) => Coverage::MissingIn(opener),
            None => Coverage::Provided,
        }
    }
}

/// The scopes each checked seed is placed in.
struct SeedPlacements(HashMap<TypeId, Vec<ScopeKey>>);

impl SeedPlacements {
    /// The factory-less placements of every `#[context]` value and every claimed type.
    fn of(components: &[ComponentDescriptor], provided: &Provided<'_>) -> Self {
        let mut placements: HashMap<TypeId, Vec<ScopeKey>> = HashMap::new();

        for component in components {
            let type_id = component.ty.type_id;
            let checked = component.construction.is_context() || provided.claims_type(type_id);
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

    /// Fails when `dependency`, required by `consumer` in `scope`, reaches a placement of a
    /// checked seed that is not provided there.
    fn check(
        &self,
        dependency: &DependencyDescriptor,
        scope: ScopeKey,
        consumer: SeedConsumer,
        provided: &Provided<'_>,
        topology: &PreparedScopeTopology,
    ) -> Result<(), SeedClaimError> {
        let required = !dependency.optional
            && !dependency.config
            && matches!(
                dependency.resolution,
                ResolutionMode::Eager | ResolutionMode::Deferred
            );
        let ty = dependency.ty.type_id;
        let Some(placements) = self.0.get(&ty) else {
            return Ok(());
        };

        if !required {
            return Ok(());
        }

        let Some(placement) = placements
            .iter()
            .find(|placement| topology.is_reachable(&scope.id(), &placement.id()))
        else {
            return Ok(());
        };

        match provided.coverage(ty, placement.id()) {
            Coverage::Provided => Ok(()),

            Coverage::MissingIn(opener) => Err(SeedClaimError::OpenerWithoutSeed {
                seed: (dependency.ty.type_name)(),
                scope: placement.name(),
                scope_id: placement.id(),
                consumer,
                opener: opener.opener.clone(),
            }),

            Coverage::Unclaimed => Err(SeedClaimError::UnprovidedSeed {
                seed: (dependency.ty.type_name)(),
                scope: placement.name(),
                scope_id: placement.id(),
                consumer,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
