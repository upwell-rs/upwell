//! Validation of deferred and `Fresh` dependencies, which resolve at use rather than at
//! construction.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, ResolutionMode, ScopeKey};

use super::ComponentRegistry;
use super::dependencies::unselectable_provider;
use super::scopes::scope_allows_by_rank;
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
        let by_type: HashMap<TypeId, ComponentDescriptor> = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect();

        for consumer in components {
            for dependency in consumer
                .dependencies()
                .into_iter()
                .filter(|dependency| dependency.resolution == ResolutionMode::Deferred)
            {
                let target = match by_type.get(&dependency.ty.type_id).copied() {
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
                            Some(selected) => {
                                model.component(selected.provider.concrete_ty.type_id)
                            }
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
                                    &dependency,
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

                if target.is_some_and(|target| target.scope.is_transient()) {
                    return Err(Error::DeferredTransientDependency(Box::new(
                        crate::error::DeferredTransientDependency {
                            component: consumer.name.to_string(),
                            component_id: consumer.id.to_string(),
                            dependency: (dependency.ty.type_name)().to_string(),
                            dependency_type: (dependency.ty.type_name)().to_string(),
                            component_scope: consumer.scope.id(),
                            dependency_scope: target
                                .expect("transient target was selected")
                                .scope
                                .id(),
                        },
                    )));
                }
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
        let by_type: HashMap<TypeId, ComponentDescriptor> = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .collect();
        let scope_of: HashMap<TypeId, ScopeKey> = components
            .iter()
            .map(|component| (component.ty.type_id, component.scope))
            .collect();

        for consumer in components {
            for dependency in consumer
                .dependencies()
                .into_iter()
                .filter(|dependency| dependency.resolution == ResolutionMode::Fresh)
            {
                let targets =
                    self.fresh_targets(model, consumer, &dependency, &by_type, can_access)?;

                for target in targets {
                    // The consumer retains a fresh instance permanently, so the
                    // target's own scope must be visible from the consumer's scope —
                    // the same lifetime rule an eager edge follows. Collection shapes
                    // skip inaccessible providers at runtime, so only validate the
                    // accessible subset.
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

                    for target_dependency in target.dependencies().into_iter().filter(|edge| {
                        edge.resolution == ResolutionMode::Eager && !edge.dynamic && !edge.config
                    }) {
                        let dependency_scopes = match scope_of.get(&target_dependency.ty.type_id) {
                            Some(scope) => vec![*scope],
                            None => self
                                .selected_dependency_scopes(
                                    model,
                                    &target,
                                    &target_dependency,
                                    can_access,
                                )?
                                .into_iter()
                                .map(|(scope, _)| scope)
                                .collect(),
                        };

                        if dependency_scopes
                            .iter()
                            .any(|scope| !can_access(consumer.scope, *scope))
                        {
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
                                    dependency_scope: dependency_scopes
                                        .iter()
                                        .find(|scope| !can_access(consumer.scope, **scope))
                                        .expect("an inaccessible scope was found")
                                        .id(),
                                },
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn fresh_targets(
        &self,
        model: &selection::ProviderSelectionModel,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        by_type: &HashMap<TypeId, ComponentDescriptor>,
        can_access: &impl Fn(ScopeKey, ScopeKey) -> bool,
    ) -> crate::Result<Vec<ComponentDescriptor>> {
        if let Some(component) = by_type.get(&dependency.ty.type_id) {
            return Ok(vec![*component]);
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

        Ok(selected
            .into_iter()
            .filter_map(|selection| model.component(selection.provider.concrete_ty.type_id))
            .collect())
    }
}
