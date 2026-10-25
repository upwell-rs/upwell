//! Validation of deferred and `Fresh` dependencies, which resolve at use rather than at
//! construction.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, ScopeKey};

use super::ComponentRegistry;
use super::dependencies::unselectable_provider;
use super::scopes::{scope_allows_by_rank, selected_dependency_scopes};
use super::selection;
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;

impl ComponentRegistry {
    /// Rejects deferred dependencies whose selected target is transient. Deferred
    /// handles retain only a weak reference, so their targets must be stored by a
    /// concrete scope after construction.
    pub fn validate_deferred_dependencies(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_deferred_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    pub(super) fn validate_deferred_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    ) -> crate::Result<()> {
        let by_type = index_by_type(components);
        let lookup = |type_id: TypeId| by_type.get(&type_id).copied();

        for consumer in components {
            for dependency in consumer.dependencies() {
                check_deferred(model, &lookup, consumer, &dependency, can_access)?;
            }
        }

        Ok(())
    }

    /// Validates that forced-fresh targets have factories and can resolve their eager
    /// dependencies from the target's construction scope.
    pub fn validate_fresh_dependencies(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_fresh_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    pub(super) fn validate_fresh_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    ) -> crate::Result<()> {
        let by_type = index_by_type(components);
        let lookup = |type_id: TypeId| by_type.get(&type_id).copied();

        for consumer in components {
            for dependency in consumer.dependencies() {
                check_fresh(model, &lookup, consumer, &dependency, can_access)?;
            }
        }

        Ok(())
    }
}

fn index_by_type(components: &[ComponentDescriptor]) -> HashMap<TypeId, ComponentDescriptor> {
    components
        .iter()
        .map(|component| (component.ty.type_id, *component))
        .collect()
}

/// Rejects a deferred edge of `consumer` whose selected target is transient. `lookup` finds
/// a registered component by type.
pub(super) fn check_deferred(
    model: &selection::ProviderSelectionModel,
    lookup: &impl Fn(TypeId) -> Option<ComponentDescriptor>,
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> crate::Result<()> {
    if dependency.resolution != ResolutionMode::Deferred {
        return Ok(());
    }

    let target = match lookup(dependency.ty.type_id) {
        Some(target) => Some(target),
        None => {
            let selected = model.select_runtime_one(
                dependency.ty.type_id,
                dependency.qualifier,
                ResolutionMode::Deferred,
                consumer.scope,
                can_access,
            );

            match selected {
                Some(selected) => model.component(selected.provider.concrete_ty.type_id),
                None if !model.has_visible_provider(
                    dependency.ty.type_id,
                    dependency.qualifier,
                    consumer.scope,
                    can_access,
                ) =>
                {
                    None
                }
                None => {
                    return Err(unselectable_provider(
                        consumer,
                        dependency,
                        model.visible_provider_ids(
                            dependency.ty.type_id,
                            dependency.qualifier,
                            ResolutionMode::Deferred,
                            consumer.scope,
                            can_access,
                        ),
                    ));
                }
            }
        }
    };

    match target {
        Some(target) if target.scope.is_transient() => Err(Error::DeferredTransientDependency(
            Box::new(crate::error::DeferredTransientDependency {
                component: consumer.name.to_string(),
                component_id: consumer.id.to_string(),
                dependency: (dependency.ty.type_name)().to_string(),
                dependency_type: (dependency.ty.type_name)().to_string(),
                component_scope: consumer.scope.id(),
                dependency_scope: target.scope.id(),
            }),
        )),
        _ => Ok(()),
    }
}

/// Requires a forced-fresh edge of `consumer` to target factory-built components whose own
/// scope and eager dependencies the consumer can access. `lookup` finds a registered
/// component by type.
pub(super) fn check_fresh(
    model: &selection::ProviderSelectionModel,
    lookup: &impl Fn(TypeId) -> Option<ComponentDescriptor>,
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> crate::Result<()> {
    if dependency.resolution != ResolutionMode::Fresh {
        return Ok(());
    }

    for target in fresh_targets(model, consumer, dependency, lookup, can_access) {
        // The consumer retains a fresh instance permanently, so the target's own scope
        // must be visible from the consumer's scope — the same lifetime rule an eager edge
        // follows. Collection shapes skip inaccessible providers at runtime, so only
        // validate the accessible subset.
        if !can_access(consumer.scope, target.scope) {
            if dependency.cardinality == Cardinality::One {
                return Err(Error::InvalidFreshDependency(Box::new(
                    crate::error::InvalidFreshDependency {
                        component: consumer.name.to_string(),
                        component_id: consumer.id.to_string(),
                        dependency: target.name.to_string(),
                        dependency_type: (target.ty.type_name)().to_string(),
                        component_scope: consumer.scope.id(),
                        dependency_scope: target.scope.id(),
                    },
                )));
            }

            continue;
        }

        if target.effective_factory()?.is_none() {
            return Err(Error::UnsupportedFreshFactory {
                component: target.name.to_string(),
                component_id: Some(target.id.to_string()),
                type_name: (target.ty.type_name)().to_string(),
            });
        }

        check_fresh_target_dependencies(model, lookup, consumer, &target, can_access)?;
    }

    Ok(())
}

/// Requires every eager dependency of a fresh `target` to live in a scope `consumer` can
/// access, since the consumer rebuilds the target from its own scope.
fn check_fresh_target_dependencies(
    model: &selection::ProviderSelectionModel,
    lookup: &impl Fn(TypeId) -> Option<ComponentDescriptor>,
    consumer: &ComponentDescriptor,
    target: &ComponentDescriptor,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> crate::Result<()> {
    for target_dependency in target
        .dependencies()
        .into_iter()
        .filter(|edge| edge.resolution == ResolutionMode::Eager && !edge.dynamic && !edge.config)
    {
        let dependency_scopes = match lookup(target_dependency.ty.type_id) {
            Some(component) => vec![component.scope],
            None => selected_dependency_scopes(model, target, &target_dependency, can_access)
                .into_iter()
                .map(|(scope, _)| scope)
                .collect(),
        };
        let inaccessible = dependency_scopes
            .iter()
            .find(|scope| !can_access(consumer.scope, **scope));

        if let Some(scope) = inaccessible {
            return Err(Error::InvalidFreshDependency(Box::new(
                crate::error::InvalidFreshDependency {
                    component: consumer.name.to_string(),
                    component_id: consumer.id.to_string(),
                    dependency: format!(
                        "{} -> {}",
                        target.name,
                        (target_dependency.ty.type_name)()
                    ),
                    dependency_type: (target_dependency.ty.type_name)().to_string(),
                    component_scope: consumer.scope.id(),
                    dependency_scope: scope.id(),
                },
            )));
        }
    }

    Ok(())
}

fn fresh_targets(
    model: &selection::ProviderSelectionModel,
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    lookup: &impl Fn(TypeId) -> Option<ComponentDescriptor>,
    can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
) -> Vec<ComponentDescriptor> {
    if let Some(component) = lookup(dependency.ty.type_id) {
        return vec![component];
    }

    let selected = match dependency.cardinality {
        Cardinality::One => model
            .select_runtime_one(
                dependency.ty.type_id,
                dependency.qualifier,
                ResolutionMode::Fresh,
                consumer.scope,
                can_access,
            )
            .into_iter()
            .collect(),
        Cardinality::Collection | Cardinality::Keyed => model.select_runtime_collection(
            dependency.ty.type_id,
            ResolutionMode::Fresh,
            consumer.scope,
            can_access,
        ),
    };

    selected
        .into_iter()
        .filter_map(|selection| model.component(selection.provider.concrete_ty.type_id))
        .collect()
}
