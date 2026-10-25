//! Per-edge resolvability: why one dependency cannot be resolved from a consumer scope.

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, ScopeId, ScopeKey};

use super::ProviderSelectionModel;
use crate::registry::scopes::scope_allows_with;

/// Why one dependency edge cannot be resolved from a consumer scope.
///
/// Returned by [`ProviderSelectionModel::dependency_fault`] for consumers that are not
/// components, such as entry points, so they can report the failure in their own terms.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DependencyFault {
    /// No component or provider of the requested type is registered.
    Missing,
    /// Providers are registered, but none lives in a scope the consumer can reach.
    Unreachable {
        /// The stable scope identity of every matching provider, in provider order.
        scopes: Vec<ScopeId>,
    },
    /// More than one reachable provider ties for a single-valued edge.
    Ambiguous {
        /// The stable component IDs of the tied providers, in provider order.
        candidates: Vec<String>,
    },
    /// A deferred edge that only transient providers could satisfy.
    DeferredTransient,
}

impl ProviderSelectionModel {
    /// Checks that `dependency` resolves from a consumer living in `consumer`.
    ///
    /// The rules are the ones component validation applies to each edge: config and dynamic
    /// edges are exempt, multi-valued and optional edges accept no provider, a qualified
    /// edge always requires a match, and a single-valued edge must select exactly one
    /// reachable provider. `can_reach` has the meaning of
    /// [`ComponentRegistry::validate_with_scope_reachability`](crate::ComponentRegistry::validate_with_scope_reachability).
    pub fn dependency_fault(
        &self,
        dependency: &DependencyDescriptor,
        consumer: ScopeKey,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> Option<DependencyFault> {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach);
        let type_id = dependency.ty.type_id;
        let qualifier = dependency.qualifier;
        let required = qualifier.is_some()
            || (dependency.cardinality.requires_provider() && !dependency.optional);

        if dependency.config || dependency.dynamic {
            return None;
        }

        if let Some(component) = self.component(type_id) {
            let reachable = dependency.resolution == ResolutionMode::Fresh
                || can_access(consumer, component.scope);

            return (!reachable).then(|| DependencyFault::Unreachable {
                scopes: vec![component.scope.id()],
            });
        }

        if !self.has_matching_provider(type_id, qualifier) {
            return required.then_some(DependencyFault::Missing);
        }

        if !self.has_visible_provider(type_id, qualifier, consumer, &can_access) {
            return required.then(|| DependencyFault::Unreachable {
                scopes: self.matching_scopes(dependency),
            });
        }

        if dependency.cardinality != Cardinality::One {
            return None;
        }

        let selected = self.select_runtime_one(
            type_id,
            qualifier,
            dependency.resolution,
            consumer,
            &can_access,
        );

        if selected.is_some() {
            return None;
        }

        let candidates = self.visible_provider_ids(
            type_id,
            qualifier,
            dependency.resolution,
            consumer,
            &can_access,
        );
        let fault = if candidates.is_empty() {
            DependencyFault::DeferredTransient
        } else {
            DependencyFault::Ambiguous { candidates }
        };

        Some(fault)
    }

    fn matching_scopes(&self, dependency: &DependencyDescriptor) -> Vec<ScopeId> {
        self.matching_providers(dependency.ty.type_id, dependency.qualifier)
            .into_iter()
            .filter_map(|provider| self.component(provider.concrete_ty.type_id))
            .map(|component| component.scope.id())
            .collect()
    }
}
