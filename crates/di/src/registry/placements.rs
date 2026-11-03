//! Placement rules: which scopes one component type may be placed in together.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Scope, ScopeId, Transient};

use super::ComponentRegistry;
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;

impl ComponentRegistry {
    /// Rejects a component type placed in two scopes where one encloses the other.
    ///
    /// `encloses(outer, inner)` must hold when `outer` is an ancestor of `inner` on at least
    /// one opening chain, the root included. Placements that share a hierarchy would give a
    /// resolution from the inner scope two candidates, so the owner's rule (2026-11-03) keeps
    /// them apart: each scope holds its own slot, and a chain reaches exactly one of them.
    pub fn validate_placement_overlap(
        &self,
        components: &[ComponentDescriptor],
        encloses: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        for placements in group_by_type(components).values() {
            for (index, left) in placements.iter().enumerate() {
                for right in &placements[index + 1..] {
                    let (outer, inner) = if encloses(left.scope.id(), right.scope.id()) {
                        (left, right)
                    } else if encloses(right.scope.id(), left.scope.id()) {
                        (right, left)
                    } else {
                        continue;
                    };

                    return Err(Error::OverlappingPlacements {
                        component: outer.name.to_string(),
                        outer: outer.scope.name(),
                        outer_id: outer.scope.id(),
                        inner: inner.scope.name(),
                        inner_id: inner.scope.id(),
                    });
                }
            }
        }

        Ok(())
    }
}

/// Rejects the placement sets that need no topology to be invalid: a transient placement
/// beside any other, and more than one seed of a type.
pub(super) fn validate_placement_set(components: &[ComponentDescriptor]) -> crate::Result<()> {
    for placements in group_by_type(components).values() {
        let transient = placements
            .iter()
            .find(|placement| placement.scope.id() == Transient::ID);
        let beside = placements
            .iter()
            .find(|placement| placement.scope.id() != Transient::ID);

        if let (Some(transient), Some(beside)) = (transient, beside) {
            return Err(Error::TransientPlacement {
                component: transient.name.to_string(),
                other: beside.scope.name(),
            });
        }

        let mut seeds = Vec::new();

        for placement in placements {
            if placement.effective_factory()?.is_none() {
                seeds.push(placement);
            }
        }

        if let [first, second, ..] = seeds.as_slice() {
            return Err(Error::AmbiguousSeedScopes {
                component: first.name.to_string(),
                first: first.scope.name(),
                second: second.scope.name(),
            });
        }
    }

    Ok(())
}

/// The components that share a type, for the types with more than one placement.
fn group_by_type(components: &[ComponentDescriptor]) -> HashMap<TypeId, Vec<&ComponentDescriptor>> {
    let mut groups: HashMap<TypeId, Vec<&ComponentDescriptor>> = HashMap::new();

    for component in components {
        groups
            .entry(component.ty.type_id)
            .or_default()
            .push(component);
    }

    groups.retain(|_, placements| placements.len() > 1);

    groups
}
