//! Dependency validation: every eager dependency resolves to exactly the providers its cardinality allows.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::{Cardinality, Scope, ScopeId};

use super::ComponentRegistry;
use super::scopes::{scope_allows_by_rank, scope_allows_with};
use super::selection::{self, SelectedDependency};
use crate::descriptors::ComponentDescriptor;
use crate::error::Error;

impl ComponentRegistry {
    /// Validates that every non-config single dependency is satisfiable by a
    /// component or trait provider.
    pub fn validate_dependencies(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let selection = self.provider_selection_model(components)?;

        self.validate_dependencies_with(components, &selection, &scope_allows_by_rank)
    }

    /// Returns the runtime targets selected for one validated dependency using
    /// caller-defined scope reachability.
    ///
    /// The predicate has the same meaning as
    /// [`validate_with_scope_reachability`](Self::validate_with_scope_reachability).
    pub fn selected_dependencies_with_scope_reachability(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &upwell_core::DependencyDescriptor,
        components: &[ComponentDescriptor],
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<Vec<SelectedDependency>> {
        selection::select(
            self,
            consumer,
            dependency,
            components,
            &|consumer, dependency| scope_allows_with(consumer, dependency, &can_reach),
        )
    }

    pub(super) fn validate_dependencies_with(
        &self,
        components: &[ComponentDescriptor],
        model: &selection::ProviderSelectionModel,
        can_access: &impl Fn(&dyn Scope, &dyn Scope) -> bool,
    ) -> crate::Result<()> {
        let available: HashSet<TypeId> = components.iter().map(|c| c.ty.type_id).collect();

        for c in components {
            for dep in c.dependencies() {
                // Config edges are validated against bindings by the config layer.
                if dep.config {
                    continue;
                }

                let dep_id = dep.ty.type_id;
                let matching = model.has_matching_provider(dep_id, dep.qualifier);
                let visible =
                    model.has_visible_provider(dep_id, dep.qualifier, c.scope, can_access);
                let must_exist =
                    dep.cardinality.requires_provider() && !dep.optional && !dep.dynamic;

                if must_exist && matching && !visible && !available.contains(&dep_id) {
                    return Err(Self::scope_unreachable_dependency(model, c, &dep));
                }

                if let Some(qualifier) = dep.qualifier {
                    let found = dep.dynamic || visible;

                    if !found {
                        return Err(Error::MissingDependency {
                            component: c.name.to_string(),
                            component_id: c.id.to_string(),
                            dependency: format!(
                                "{} (qualifier `{qualifier}`)",
                                (dep.ty.type_name)()
                            ),
                            type_name: (dep.ty.type_name)().to_string(),
                        });
                    }

                    continue;
                }

                if dep.cardinality == Cardinality::One && !dep.dynamic && matching && visible {
                    let selected = model.select_runtime_one(
                        dep_id,
                        dep.qualifier,
                        dep.resolution,
                        c.scope,
                        can_access,
                    );

                    if let Some(selected) = selected {
                        if let Some(component) =
                            model.component(selected.provider.concrete_ty.type_id)
                        {
                            crate::observability::provider_selection(
                                c,
                                &dep,
                                selected.provider,
                                component,
                                selected.reason,
                                selected.stage,
                            );
                        }
                    } else {
                        crate::observability::selection_absent(c, &dep);

                        return Err(unselectable_provider(
                            c,
                            &dep,
                            model.visible_provider_ids(
                                dep_id,
                                dep.qualifier,
                                dep.resolution,
                                c.scope,
                                can_access,
                            ),
                        ));
                    }
                }

                if must_exist && !available.contains(&dep_id) && (!matching || !visible) {
                    return Err(Error::MissingDependency {
                        component: c.name.to_string(),
                        component_id: c.id.to_string(),
                        dependency: (dep.ty.type_name)().to_string(),
                        type_name: (dep.ty.type_name)().to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Rejects duplicate `(trait, qualifier)` providers within one scope: a
    /// qualifier selects the first registered match, so two providers sharing a
    /// qualifier in the same scope would resolve by build order — descriptor-order
    /// dependent and effectively arbitrary. Distinct scopes may legitimately share
    /// a qualifier (the closer scope wins by design).
    pub fn validate_provider_qualifiers(
        &self,
        components: &[ComponentDescriptor],
    ) -> crate::Result<()> {
        let scope_of: HashMap<TypeId, ScopeId> = components
            .iter()
            .map(|component| (component.ty.type_id, component.scope.id()))
            .collect();
        let mut seen: HashSet<(TypeId, &str, ScopeId)> = HashSet::new();

        for provider in &self.providers {
            let Some(scope) = scope_of.get(&provider.concrete_ty.type_id).copied() else {
                continue;
            };

            if !seen.insert((provider.trait_ty.type_id, provider.qualifier, scope)) {
                return Err(Error::DuplicateProviderQualifier {
                    trait_name: (provider.trait_ty.type_name)().to_string(),
                    trait_type: (provider.trait_ty.type_name)().to_string(),
                    qualifier: provider.qualifier.to_string(),
                    scope,
                });
            }
        }

        Ok(())
    }
}

/// The error for a single-provider dependency no provider can be selected for: a tie
/// among `candidates`, or, when deferred resolution filtered every visible provider out,
/// a deferred dependency that only transient providers could satisfy.
pub(super) fn unselectable_provider(
    consumer: &ComponentDescriptor,
    dependency: &upwell_core::DependencyDescriptor,
    candidates: Vec<String>,
) -> Error {
    let type_name = (dependency.ty.type_name)().to_string();

    if candidates.is_empty() {
        return Error::DeferredTransientDependency(Box::new(
            crate::error::DeferredTransientDependency {
                component: consumer.name.to_string(),
                component_id: consumer.id.to_string(),
                dependency: type_name.clone(),
                dependency_type: type_name,
                component_scope: consumer.scope.id(),
                dependency_scope: <upwell_core::Transient as upwell_core::StaticScope>::ID,
            },
        ));
    }

    Error::AmbiguousProvider {
        component_id: Some(consumer.id.to_string()),
        type_name,
        candidates,
    }
}
