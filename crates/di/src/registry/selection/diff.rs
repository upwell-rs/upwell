//! The types whose runtime selection differs between two selection models, so plans that
//! selected none of them carry over to a new generation.

use std::any::TypeId;
use std::collections::HashSet;

use upwell_core::ScopeId;

use super::ProviderSelectionModel;
use crate::descriptors::ProviderDescriptor;

/// What runtime selection reads of one provider: its component, the scope that component
/// lives in, and the attributes selection ranks it by.
#[derive(Eq, PartialEq)]
struct ProviderFingerprint {
    concrete: TypeId,
    scope: Option<ScopeId>,
    qualifier: &'static str,
    primary: bool,
    priority: i64,
}

impl ProviderSelectionModel {
    /// The types a runtime selection may resolve differently in `other` than here: every
    /// trait whose providers differ in order, qualifier, primacy, priority or the scope of
    /// their component, and every component type present in one model only or in another
    /// scope.
    ///
    /// A selection of a type outside this set resolves to the same providers in both
    /// models, given the same scope reachability.
    pub fn changed_selections(&self, other: &Self) -> HashSet<TypeId> {
        let mut changed = HashSet::new();
        let traits = self.by_trait.keys().chain(other.by_trait.keys());
        let components = self.components.keys().chain(other.components.keys());

        for trait_id in traits {
            if !self
                .fingerprints(*trait_id)
                .eq(other.fingerprints(*trait_id))
            {
                changed.insert(*trait_id);
            }
        }

        for type_id in components {
            let scope = |model: &Self| {
                model
                    .placements(*type_id)
                    .iter()
                    .map(|component| component.scope.id())
                    .collect::<Vec<_>>()
            };

            if scope(self) != scope(other) {
                changed.insert(*type_id);
            }
        }

        changed
    }

    fn fingerprints(&self, trait_id: TypeId) -> impl Iterator<Item = ProviderFingerprint> + '_ {
        self.providers_for_trait(trait_id)
            .iter()
            .map(|provider| self.fingerprint(provider))
    }

    fn fingerprint(&self, provider: &ProviderDescriptor) -> ProviderFingerprint {
        ProviderFingerprint {
            concrete: provider.concrete_ty.type_id,
            scope: self
                .component(provider.concrete_ty.type_id)
                .map(|component| component.scope.id()),
            qualifier: provider.qualifier,
            primary: provider.primary,
            priority: provider.priority,
        }
    }
}
