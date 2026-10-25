//! Per-edge resolvability: why one dependency cannot be resolved from a consumer scope.

use upwell_core::{DependencyDescriptor, ScopeId, ScopeKey, TypeDescriptor};

use super::ProviderSelectionModel;
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;
use crate::registry::dependencies::check_dependency;
use crate::registry::resolution::{check_deferred, check_fresh};
use crate::registry::scopes::{check_scope, scope_allows_with};

/// Why one dependency edge cannot be resolved from a consumer scope.
///
/// Returned by [`ProviderSelectionModel::dependency_fault`] for consumers that are not
/// components, such as entry points, so they can report the failure in their own terms.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DependencyFault {
    /// No component or provider of the requested type is registered.
    Missing,
    /// The selected target lives in a scope the consumer cannot reach.
    Unreachable {
        /// The stable scope identity of every unreachable target, in provider order.
        scopes: Vec<ScopeId>,
    },
    /// More than one reachable provider ties for a single-valued edge.
    Ambiguous {
        /// The stable component IDs of the tied providers, in provider order.
        candidates: Vec<String>,
    },
    /// A deferred edge that only transient providers could satisfy.
    DeferredTransient,
    /// A forced-fresh target, or one of its eager dependencies, lives in a scope the
    /// consumer cannot reach.
    FreshUnreachable {
        /// The unreachable scope.
        scope: ScopeId,
    },
    /// A forced-fresh target has no factory to rebuild it with.
    FreshWithoutFactory {
        /// The target's stable component ID.
        component: String,
    },
}

/// The stand-in consumer an edge is checked as: per-edge validation reads only a
/// consumer's identity and scope, never its own dependencies.
struct EdgeConsumer;

impl ProviderSelectionModel {
    /// Checks that `dependency` resolves from a consumer living in `consumer`.
    ///
    /// Runs exactly the per-edge checks component validation runs, in the same order:
    /// provider existence and selection, deferred targets, forced-fresh targets, then the
    /// target scopes. `can_reach` has the meaning of
    /// [`ComponentRegistry::validate_with_scope_reachability`](crate::ComponentRegistry::validate_with_scope_reachability).
    /// Failures with no per-edge meaning, such as a target with ambiguous factories, are
    /// returned as errors.
    pub fn dependency_fault(
        &self,
        dependency: &DependencyDescriptor,
        consumer: ScopeKey,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<Option<DependencyFault>> {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, &can_reach);
        let lookup = |type_id| self.component(type_id);
        let is_component = |type_id| self.component(type_id).is_some();
        let consumer = ComponentDescriptor::manual(
            "edge-consumer",
            "edge consumer",
            TypeDescriptor::of::<EdgeConsumer>("EdgeConsumer"),
            consumer,
        );
        let checked = check_dependency(self, &is_component, &consumer, dependency, &can_access)
            .and_then(|()| check_deferred(self, &lookup, &consumer, dependency, &can_access))
            .and_then(|()| check_fresh(self, &lookup, &consumer, dependency, &can_access))
            .and_then(|()| check_scope(self, &lookup, &consumer, dependency, &can_access));

        match checked {
            Ok(()) => Ok(None),
            Err(error) => fault(error).map(Some),
        }
    }
}

/// Maps a per-edge validation error to its fault, returning any other error unchanged.
fn fault(error: Error) -> crate::Result<DependencyFault> {
    let fault = match error {
        Error::MissingDependency { .. } => DependencyFault::Missing,
        Error::ScopeUnreachableDependency(details) => DependencyFault::Unreachable {
            scopes: details
                .providers
                .iter()
                .map(|provider| provider.scope_id)
                .collect(),
        },
        Error::ScopeViolation(violation) => DependencyFault::Unreachable {
            scopes: vec![violation.dependency_scope_id],
        },
        Error::AmbiguousProvider { candidates, .. } => DependencyFault::Ambiguous { candidates },
        Error::DeferredTransientDependency(_) => DependencyFault::DeferredTransient,
        Error::InvalidFreshDependency(fresh) => DependencyFault::FreshUnreachable {
            scope: fresh.dependency_scope,
        },
        Error::UnsupportedFreshFactory {
            component,
            component_id,
            ..
        } => DependencyFault::FreshWithoutFactory {
            component: component_id.unwrap_or(component),
        },
        other => return Err(other),
    };

    Ok(fault)
}
