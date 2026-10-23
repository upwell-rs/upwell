//! Scope validation: each dependency's scope must be reachable from its consumer's.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, ResolutionMode, Scope, ScopeId, Singleton};

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

        self.validate_scopes_with(components, &selection, &scope_allows_by_rank)
    }

    pub(super) fn validate_scopes_with(
        &self,
        components: &[ComponentDescriptor],
        selection: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let scope_of: HashMap<TypeId, &'static dyn Scope> =
            components.iter().map(|c| (c.ty.type_id, c.scope)).collect();

        for c in components {
            for dep in c.dependencies() {
                // Config edges resolve against external bindings; dynamic edges are
                // runtime-provided. Neither participates in the scope rule.
                if dep.dynamic || dep.config {
                    continue;
                }

                let dep_id = dep.ty.type_id;

                let dep_scopes = match scope_of.get(&dep_id) {
                    Some(scope) => vec![(*scope, (dep.ty.type_name)())],
                    None => self.selected_dependency_scopes(selection, c, &dep, can_access)?,
                };

                for (dep_scope, dep_name) in dep_scopes {
                    if dep.resolution != ResolutionMode::Fresh && !can_access(c.scope, dep_scope) {
                        return Err(Error::ScopeViolation(Box::new(
                            crate::error::ScopeViolation {
                                component: c.name.to_string(),
                                component_id: c.id.to_string(),
                                dependency: dep_name.to_string(),
                                dependency_type: (dep.ty.type_name)().to_string(),
                                component_scope: c.scope.name(),
                                component_scope_id: c.scope.id(),
                                dependency_scope: dep_scope.name(),
                                dependency_scope_id: dep_scope.id(),
                            },
                        )));
                    }
                }
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
            .filter_map(|provider| {
                let component = model.component(provider.concrete_ty.type_id)?;

                Some(crate::error::ScopeUnreachableProvider {
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

    pub(super) fn selected_dependency_scopes(
        &self,
        model: &selection::ProviderSelectionModel,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<Vec<(&'static dyn Scope, &'static str)>> {
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

        Ok(selected
            .into_iter()
            .filter_map(|selection| {
                let component = model.component(selection.provider.concrete_ty.type_id)?;

                Some((
                    component.scope,
                    (selection.provider.concrete_ty.type_name)(),
                ))
            })
            .collect())
    }
}

/// Whether a `consumer`-scoped component may hold a `dependency`-scoped one under
/// the topology-neutral legacy rank model.
pub(super) fn scope_allows_by_rank(consumer: &dyn Scope, dependency: &dyn Scope) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton.id();
    }

    dependency.rank() >= consumer.rank()
}

pub(super) fn scope_allows_with(
    consumer: &dyn Scope,
    dependency: &dyn Scope,
    can_reach: &impl Fn(ScopeId, ScopeId) -> bool,
) -> bool {
    if dependency.is_transient() {
        return true;
    }

    if consumer.is_transient() {
        return dependency.id() == Singleton.id();
    }

    can_reach(consumer.id(), dependency.id())
}
