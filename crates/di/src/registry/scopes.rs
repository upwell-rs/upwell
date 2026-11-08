//! Scope validation: each dependency's scope must be reachable from its consumer's.

use upwell_core::{Cardinality, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton};

use super::ComponentRegistry;
use super::selection;
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;

impl ComponentRegistry {
    /// Enforces the captive-dependency rule: a non-transient component may depend
    /// only on equal-or-longer-lived non-transient components. Checked against
    /// [`Scope::rank`], not by matching each label.
    pub fn validate_scopes(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_scopes_with(
            components,
            &selection,
            &scope_allows_by_rank,
            &never_encloses,
        )
    }

    pub(super) fn validate_scopes_with(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
        may_enclose: &impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<()> {
        for consumer in components {
            for dependency in consumer.dependencies() {
                check_scope(selection, consumer, &dependency, can_access, may_enclose)?;
            }
        }

        Ok(())
    }
}

/// Requires the scope of every target one dependency edge of `consumer` resolves to to be
/// accessible from the consumer's scope. `lookup` finds a registered component by type.
///
/// Three kinds of edge are exempt: config edges resolve against external bindings, dynamic
/// edges are runtime-provided, and invocation-context edges (`Current<T>`, `Scoped<S, T>`)
/// retain nothing, so they cannot hold a shorter-lived value captive.
///
/// An optional edge may also name a target the consumer cannot reach on every chain, when the
/// target's scope encloses the consumer's on at least one (`may_enclose(target, consumer)`): an
/// HTTP request that may open under the root or under an application's session takes the
/// session's component as `Option<Arc<T>>`, which is `None` on a chain without the session. An
/// enclosing scope outlives the consumer, so it cannot hold a value captive. A scope that never
/// encloses the consumer, such as a sibling, is still rejected: the edge could never be present.
pub(super) fn check_scope(
    selection: &selection::ProviderSelectionModel,
    consumer: &ComponentDescriptor,
    dep: &upwell_core::DependencyDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    may_enclose: &impl Fn(ScopeId, ScopeId) -> bool,
) -> crate::Result<()> {
    if dep.dynamic || dep.config || dep.resolution == ResolutionMode::Current {
        return Ok(());
    }

    let dep_scopes = match selection.component_from(dep.ty.type_id, consumer.scope, can_access) {
        Some(component) => vec![(component.scope, (dep.ty.type_name)())],
        None => selected_dependency_scopes(selection, consumer, dep, can_access),
    };

    for (dep_scope, dep_name) in dep_scopes {
        if dep.resolution != ResolutionMode::Fresh
            && !can_access(consumer.scope, dep_scope)
            && !(dep.optional && encloses_on_some_chain(dep_scope, consumer.scope, may_enclose))
        {
            return Err(Error::ScopeViolation(Box::new(
                crate::error::ScopeViolation {
                    component: consumer.name.to_string(),
                    component_id: consumer.id.to_string(),
                    dependency: dep_name.to_string(),
                    dependency_type: (dep.ty.type_name)().to_string(),
                    component_scope: consumer.scope.name(),
                    component_scope_id: consumer.scope.id(),
                    dependency_scope: dep_scope.name(),
                    dependency_scope_id: dep_scope.id(),
                },
            )));
        }
    }

    Ok(())
}

pub(super) fn scope_unreachable_dependency(
    model: &selection::ProviderSelectionModel,
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
) -> Error {
    let providers = model
        .matching_providers(dependency.ty.type_id, dependency.qualifier)
        .into_iter()
        .flat_map(|provider| {
            model
                .placements(provider.concrete_ty.type_id)
                .iter()
                .map(move |component| crate::error::ScopeUnreachableProvider {
                    component: component.name.to_string(),
                    component_id: component.id.to_string(),
                    component_type: (component.ty.type_name)().to_string(),
                    scope: component.scope.name().to_string(),
                    scope_id: component.scope.id(),
                    qualifier: provider.qualifier.to_string(),
                })
        })
        .collect();
    let dependency_name = dependency.qualifier.map_or_else(
        || dependency.name.to_string(),
        |qualifier| format!("{} (qualifier `{qualifier}`)", dependency.name),
    );

    Error::ScopeUnreachableDependency(Box::new(crate::error::ScopeUnreachableDependency {
        component: consumer.name.to_string(),
        component_id: consumer.id.to_string(),
        dependency: dependency_name,
        dependency_type: (dependency.ty.type_name)().to_string(),
        component_scope: consumer.scope.name().to_string(),
        component_scope_id: consumer.scope.id(),
        providers,
    }))
}

/// The scope and provider type name of every target `dependency` selects from `consumer`.
pub(super) fn selected_dependency_scopes(
    model: &selection::ProviderSelectionModel,
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> Vec<(ScopeKey, &'static str)> {
    let selected = match dependency.cardinality {
        Cardinality::One => model
            .select_runtime_one(
                dependency.ty.type_id,
                dependency.qualifier,
                dependency.resolution,
                consumer.scope,
                can_access,
            )
            .into_iter()
            .collect(),
        Cardinality::Collection => model.select_runtime_collection(
            dependency.ty.type_id,
            dependency.resolution,
            consumer.scope,
            can_access,
        ),
        Cardinality::Keyed => model.select_runtime_keyed(
            dependency.ty.type_id,
            dependency.resolution,
            consumer.scope,
            can_access,
        ),
    };

    selected
        .into_iter()
        .filter_map(|selection| {
            let component = model.component_from(
                selection.provider.concrete_ty.type_id,
                consumer.scope,
                can_access,
            )?;

            Some((
                component.scope,
                (selection.provider.concrete_ty.type_name)(),
            ))
        })
        .collect()
}

/// Whether `scope` is open around `consumer` on at least one chain `consumer` opens in, neither
/// of them transient.
fn encloses_on_some_chain(
    scope: ScopeKey,
    consumer: ScopeKey,
    may_enclose: &impl Fn(ScopeId, ScopeId) -> bool,
) -> bool {
    !scope.is_transient() && !consumer.is_transient() && may_enclose(scope.id(), consumer.id())
}

/// The enclosure of a model without a topology: no scope encloses another beyond what scope
/// access already allows.
pub(crate) fn never_encloses(_ancestor: ScopeId, _descendant: ScopeId) -> bool {
    false
}

/// Whether a `consumer`-scoped component may hold a `dependency`-scoped one under
/// the topology-neutral legacy rank model.
pub(super) fn scope_allows_by_rank(consumer: ScopeKey, dependency: ScopeKey) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton::ID;
    }

    dependency.rank() >= consumer.rank()
}

pub(crate) fn scope_allows_with(
    consumer: ScopeKey,
    dependency: ScopeKey,
    can_reach: &impl Fn(ScopeId, ScopeId) -> bool,
) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton::ID;
    }

    can_reach(consumer.id(), dependency.id())
}
